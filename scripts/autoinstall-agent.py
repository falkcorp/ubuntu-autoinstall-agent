#!/usr/bin/env python3
# file: scripts/autoinstall-agent.py
# version: 1.4.0
# guid: 7b6e4a2c-9f1d-4e8a-b3c6-2d5f8a1c9e07
# last-edited: 2026-07-10
"""
autoinstall-agent: HTTP service on port 25000
Handles webhook events, iPXE flips, cert issuance, MAC-based auth with approval,
YubiKey registry for centralized key management, and tang server tracking.

Security model:
- MACs must be pre-registered via /api/register (admin action)
- After registration, installs are PENDING until admin approves via /api/approve/<mac>
- On first boot, machine sends MAC + TPM EK public key hash for binding
- All sensitive endpoints (certs, flip to install) require approved status
- Flip to boot-local-disk on success is allowed for any registered MAC
- YubiKeys are registered and approved centrally; approved keys propagate to all servers

Deployed copy lives on the netboot server (172.16.2.30) at
/var/www/html/cloud-init/scripts/autoinstall-agent.py, run by
autoinstall-agent.service (user cockroach-autoinstall). This file in the repo
is a tracked mirror for version control/review — after editing, scp it to the
server and `sudo systemctl restart autoinstall-agent` to deploy.
"""
import json, os, re, time, base64, hashlib, secrets
from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs
from datetime import datetime
import subprocess, tempfile, shutil

IPXE_BOOT_DIR = "/var/www/html/ipxe/boot"
CLOUD_INIT_BASE = "/var/www/html/cloud-init"
UAA_BINARY_PATH = "/var/www/html/uaa/uaa-amd64"
LOG_DIR = "/var/log/cockroach-autoinstall"
EVENTS_LOG = f"{LOG_DIR}/events.jsonl"
FILES_DIR = f"{LOG_DIR}/files"
REGISTRY_FILE = f"{LOG_DIR}/registry.json"
YUBIKEY_REGISTRY_FILE = f"{LOG_DIR}/yubikey-registry.json"
TANG_REGISTRY_FILE = f"{LOG_DIR}/tang-registry.json"

CA_CRT = "/var/lib/cockroach-autoinstall/.cockroach-ca/ca.crt"
CA_KEY = "/var/lib/cockroach-autoinstall/.cockroach-ca/ca.key"

os.makedirs(LOG_DIR, exist_ok=True)
os.makedirs(FILES_DIR, exist_ok=True)

def agent_binary_status(path):
    """stat the served uaa binary. ABSENT file/dir is the normal, handled
    case (build-musl.sh only PRINTS the deploy hint) — never an exception."""
    info = {"path": path, "present": False, "size": None, "mtime": None}
    try:
        st = os.stat(path)
    except OSError:
        return info
    info["present"] = True
    info["size"] = st.st_size
    info["mtime"] = datetime.utcfromtimestamp(st.st_mtime).strftime("%Y-%m-%dT%H:%M:%SZ")
    return info

# ── Machine Registry ─────────────────────────────────────────────────────────
def load_registry():
    try:
        return json.load(open(REGISTRY_FILE))
    except:
        return {}

def save_registry(reg):
    tmp = REGISTRY_FILE + ".tmp"
    json.dump(reg, open(tmp, "w"), indent=2)
    os.replace(tmp, REGISTRY_FILE)

def normalize_mac(mac):
    return mac.lower().replace("-", ":").replace(".", ":")

def mac_to_hex(mac):
    return mac.lower().replace(":", "").replace("-", "")

# ── YubiKey Registry ─────────────────────────────────────────────────────────
def load_yk_registry():
    try:
        return json.load(open(YUBIKEY_REGISTRY_FILE))
    except:
        return {}

def save_yk_registry(reg):
    tmp = YUBIKEY_REGISTRY_FILE + ".tmp"
    json.dump(reg, open(tmp, "w"), indent=2)
    os.replace(tmp, YUBIKEY_REGISTRY_FILE)

# ── Tang Registry ─────────────────────────────────────────────────────────────
def load_tang_registry():
    try:
        return json.load(open(TANG_REGISTRY_FILE))
    except:
        return {}

def save_tang_registry(reg):
    tmp = TANG_REGISTRY_FILE + ".tmp"
    json.dump(reg, open(tmp, "w"), indent=2)
    os.replace(tmp, TANG_REGISTRY_FILE)

# ── iPXE ────────────────────────────────────────────────────────────────────
def find_ipxe_file_by_hostname(hostname):
    reg = load_registry()
    for mac, entry in reg.items():
        if entry.get("hostname") == hostname:
            return os.path.join(IPXE_BOOT_DIR, f"mac-{mac_to_hex(mac)}.ipxe")
    for f in os.listdir(IPXE_BOOT_DIR):
        if f.endswith(".ipxe"):
            content = open(os.path.join(IPXE_BOOT_DIR, f)).read()
            if f"set hostname {hostname}" in content:
                return os.path.join(IPXE_BOOT_DIR, f)
    return None

def webhook_should_flip(data):
    """True only for FINAL successful-install webhook payloads.

    cloud-init reporting.sh posts status "finished"/"complete" (always final).
    The Rust uaa installer posts event_type "status_update" with status
    "running" (start + per-phase), "failed", and "success" (final at
    progress 100) — a status_update may flip only on a final result.
    """
    status = data.get("status", "")
    name = data.get("name", "")
    if name and status in ("finished", "complete", "success"):
        if data.get("event_type") == "status_update":
            return status == "success" or data.get("progress") == 100
        return True
    return False

def flip_ipxe(hostname, target="boot-local-disk"):
    path = find_ipxe_file_by_hostname(hostname)
    if not path or not os.path.exists(path):
        return False, f"No iPXE file found for {hostname}"
    content = open(path).read()
    new = re.sub(r"set menu-default \S+", f"set menu-default {target}", content)
    open(path, "w").write(new)
    return True, f"Flipped {hostname} to {target}"

# ── Auto-resolved cloud-init seed (IP -> ARP/NDP -> MAC -> hexmac dir) ────────
# Lets one generic ds=nocloud;s=http://172.16.2.30:25000/autoinstall/ kernel
# cmdline (baked once into any USB/ISO/netboot config) resolve to the correct
# per-machine seed with no per-machine URL and no client self-reporting: the
# DHCP/ARP exchange the client already did to reach us on the wire is the only
# "inventory" this needs, since the kernel must resolve a peer's MAC via
# ARP/NDP before it can even complete the TCP handshake we're already inside.
def mac_from_neighbor_table(ip):
    try:
        out = subprocess.run(
            ["ip", "neigh", "show", ip], capture_output=True, text=True, timeout=3
        ).stdout
        m = re.search(r"lladdr ([0-9a-fA-F:]+)", out)
        return m.group(1) if m else None
    except Exception:
        return None

def resolve_cloud_init_dir(client_ip):
    mac = mac_from_neighbor_table(client_ip)
    if not mac:
        return None, None
    hexmac = mac_to_hex(mac)
    path = os.path.join(CLOUD_INIT_BASE, hexmac)
    return hexmac, (path if os.path.isdir(path) else None)

# ── Certs ────────────────────────────────────────────────────────────────────
def generate_certs(hostname, ip):
    tmpdir = tempfile.mkdtemp()
    try:
        shutil.copy(CA_CRT, os.path.join(tmpdir, "ca.crt"))
        result = subprocess.run([
            "cockroach", "cert", "create-node",
            ip, hostname, f"{hostname}.jf.local", "localhost", "127.0.0.1",
            f"--certs-dir={tmpdir}", f"--ca-key={CA_KEY}"
        ], capture_output=True, text=True)
        if result.returncode != 0:
            return None, result.stderr
        certs = {}
        for fname in ["ca.crt", "node.crt", "node.key"]:
            certs[fname] = base64.b64encode(open(os.path.join(tmpdir, fname), "rb").read()).decode()
        return certs, None
    except Exception as e:
        return None, str(e)
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)

# ── Logging ──────────────────────────────────────────────────────────────────
def log_event(data):
    with open(EVENTS_LOG, "a") as f:
        f.write(json.dumps({"received_at": int(time.time()), **data}) + "\n")

def log(msg):
    print(f"[{datetime.now().strftime('%Y-%m-%d %H:%M:%S')}] {msg}", flush=True)

# ── Handler ──────────────────────────────────────────────────────────────────
class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args): pass

    def send_json(self, code, data):
        body = json.dumps(data).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", len(body))
        self.end_headers()
        self.wfile.write(body)

    def get_body(self):
        length = int(self.headers.get("Content-Length", 0))
        return self.rfile.read(length)

    def do_GET(self):
        parsed = urlparse(self.path)
        path = parsed.path
        qs = parse_qs(parsed.query)

        # ── Machine certs ──
        m = re.match(r"^/api/certs/(.+)$", path)
        if m:
            hostname = m.group(1)
            ip = qs.get("ip", ["127.0.0.1"])[0]
            mac = qs.get("mac", [""])[0]
            reg = load_registry()
            entry = reg.get(normalize_mac(mac)) if mac else None
            if not entry:
                for e in reg.values():
                    if e.get("hostname") == hostname:
                        entry = e
                        break
            if not entry:
                log(f"CERTS DENIED {hostname} ({ip}) - not registered")
                self.send_json(403, {"ok": False, "error": "Not registered. Run register-len-server.sh first."})
                return
            if entry.get("status") != "approved":
                log(f"CERTS DENIED {hostname} - status={entry.get('status')} (needs approval)")
                self.send_json(403, {"ok": False, "error": f"Pending approval. Status: {entry.get('status')}."})
                return
            certs, err = generate_certs(hostname, ip)
            if err:
                self.send_json(500, {"ok": False, "error": err})
                return
            log(f"CERTS issued for {hostname} ({ip})")
            self.send_json(200, {"ok": True, "certs": certs})
            return

        # ── iPXE flip ──
        m = re.match(r"^/api/flip/(.+)$", path)
        if m:
            hostname = m.group(1)
            target = qs.get("target", ["boot-local-disk"])[0]
            if target == "custom-autoinstall":
                reg = load_registry()
                entry = next((e for e in reg.values() if e.get("hostname") == hostname), None)
                if not entry or entry.get("status") != "approved":
                    log(f"FLIP TO INSTALL DENIED {hostname} - not approved")
                    self.send_json(403, {"ok": False, "error": "Flip to reinstall requires approved status"})
                    return
            ok, msg = flip_ipxe(hostname, target)
            log(f"FLIP {hostname} -> {target}: {msg}")
            self.send_json(200 if ok else 404, {"ok": ok, "message": msg})
            return

        # ── Approve machine ──
        m = re.match(r"^/api/approve/(.+)$", path)
        if m:
            mac = normalize_mac(m.group(1))
            reg = load_registry()
            if mac not in reg:
                self.send_json(404, {"ok": False, "error": "MAC not registered"})
                return
            reg[mac]["status"] = "approved"
            reg[mac]["approved_at"] = int(time.time())
            save_registry(reg)
            log(f"APPROVED {mac} ({reg[mac].get('hostname')})")
            self.send_json(200, {"ok": True, "message": f"Approved {mac}", "entry": reg[mac]})
            return

        # ── Deregister machine ──
        m = re.match(r"^/api/deregister/(.+)$", path)
        if m:
            mac = normalize_mac(m.group(1))
            reg = load_registry()
            if mac not in reg:
                self.send_json(404, {"ok": False, "error": "MAC not registered"})
                return
            hostname = reg[mac].get("hostname")
            del reg[mac]
            save_registry(reg)
            log(f"DEREGISTERED {mac} ({hostname})")
            self.send_json(200, {"ok": True, "message": f"Deregistered {mac} ({hostname})"})
            return

        # ── Health / liveness ──
        if path == "/api/health":
            reg = load_registry()
            self.send_json(200, {
                "status": "ok",
                "registry_hosts": len(reg),
                "registry_approved": sum(1 for e in reg.values() if e.get("status") == "approved"),
                "yubikeys": len(load_yk_registry()),
                "tang_servers": len(load_tang_registry()),
                "agent_binary": agent_binary_status(UAA_BINARY_PATH),
            })
            return

        # ── Machine registry ──
        if path == "/api/registry":
            self.send_json(200, load_registry())
            return

        # ── Events ──
        if path == "/api/events":
            try:
                lines = open(EVENTS_LOG).readlines()[-50:]
                self.send_json(200, [json.loads(l) for l in lines])
            except:
                self.send_json(200, [])
            return

        # ════════════════════════════════════════════════════════════════════
        # ── YubiKey Registry ─────────────────────────────────────────────
        # ════════════════════════════════════════════════════════════════════

        # GET /api/yubikeys — list all registered YubiKeys (status, fingerprint, comment)
        if path == "/api/yubikeys":
            yk = load_yk_registry()
            # Strip raw pubkey blobs from listing for brevity; use /pubkey endpoint for those
            summary = {fp: {k: v for k, v in entry.items() if k not in ("gpg_pubkey",)} for fp, entry in yk.items()}
            self.send_json(200, summary)
            return

        # GET /api/yubikeys/ssh-keys — all approved SSH public keys (for authorized_keys refresh)
        if path == "/api/yubikeys/ssh-keys":
            yk = load_yk_registry()
            keys = [e["ssh_pubkey"] for e in yk.values() if e.get("status") == "approved" and e.get("ssh_pubkey")]
            self.send_json(200, {"keys": keys})
            return

        # GET /api/yubikeys/approve/<fingerprint> — approve a pending YubiKey
        m = re.match(r"^/api/yubikeys/approve/(.+)$", path)
        if m:
            fp = m.group(1).upper()
            yk = load_yk_registry()
            if fp not in yk:
                self.send_json(404, {"ok": False, "error": "Fingerprint not registered"})
                return
            yk[fp]["status"] = "approved"
            yk[fp]["approved_at"] = int(time.time())
            save_yk_registry(yk)
            log(f"YUBIKEY APPROVED {fp} ({yk[fp].get('comment', '')})")
            self.send_json(200, {"ok": True, "fingerprint": fp, "entry": {k: v for k, v in yk[fp].items() if k != "gpg_pubkey"}})
            return

        # GET /api/yubikeys/<fingerprint>/pubkey — get GPG public key armored block
        m = re.match(r"^/api/yubikeys/([A-F0-9]+)/pubkey$", path)
        if m:
            fp = m.group(1).upper()
            yk = load_yk_registry()
            if fp not in yk or not yk[fp].get("gpg_pubkey"):
                self.send_json(404, {"ok": False, "error": "No GPG key for that fingerprint"})
                return
            if yk[fp].get("status") != "approved":
                self.send_json(403, {"ok": False, "error": "YubiKey not approved"})
                return
            body = yk[fp]["gpg_pubkey"].encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/pgp-keys")
            self.send_header("Content-Length", len(body))
            self.end_headers()
            self.wfile.write(body)
            return

        # GET /api/yubikeys/revoke/<fingerprint> — revoke (set status=revoked)
        m = re.match(r"^/api/yubikeys/revoke/(.+)$", path)
        if m:
            fp = m.group(1).upper()
            yk = load_yk_registry()
            if fp not in yk:
                self.send_json(404, {"ok": False, "error": "Fingerprint not registered"})
                return
            yk[fp]["status"] = "revoked"
            yk[fp]["revoked_at"] = int(time.time())
            save_yk_registry(yk)
            log(f"YUBIKEY REVOKED {fp} ({yk[fp].get('comment', '')})")
            self.send_json(200, {"ok": True, "message": f"Revoked {fp}"})
            return

        # ════════════════════════════════════════════════════════════════════
        # ── Tang Registry ─────────────────────────────────────────────────
        # ════════════════════════════════════════════════════════════════════

        # GET /api/tang/servers — list all registered tang servers
        if path == "/api/tang/servers":
            tang = load_tang_registry()
            self.send_json(200, tang)
            return

        # ════════════════════════════════════════════════════════════════════
        # ── Auto-resolved cloud-init seed ─────────────────────────────────
        # GET /autoinstall/{user-data,meta-data,vendor-data,network-config}
        # ════════════════════════════════════════════════════════════════════
        m = re.match(r"^/autoinstall/(user-data|meta-data|vendor-data|network-config)$", path)
        if m:
            filename = m.group(1)
            client_ip = self.client_address[0]
            hexmac, dir_path = resolve_cloud_init_dir(client_ip)
            if not hexmac:
                log(f"AUTOINSTALL {filename} DENIED {client_ip} - no ARP/NDP neighbor entry")
                self.send_response(404)
                self.end_headers()
                return
            if not dir_path:
                log(f"AUTOINSTALL {filename} DENIED {client_ip} (hexmac={hexmac}) - no cloud-init dir registered")
                self.send_response(404)
                self.end_headers()
                return
            file_path = os.path.join(dir_path, filename)
            body = open(file_path, "rb").read() if os.path.isfile(file_path) else b""
            log(f"AUTOINSTALL {filename} -> {client_ip} (hexmac={hexmac})")
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", len(body))
            self.end_headers()
            self.wfile.write(body)
            return

        # ════════════════════════════════════════════════════════════════════
        # ── Auto-resolved installer config (USB / netboot bootstrap) ──────
        # GET /autoinstall/uaa-config
        # Serves the per-host InstallationConfig (<hexmac>/uaa.yaml) resolved
        # the same MAC-as-identity way as the cloud-init seed above. Unlike the
        # seed files, a MISSING uaa.yaml is a hard 404 (never an empty 200):
        # the USB bootstrap must fail loudly at fetch time, not hand an empty
        # config to `uaa install`. Place uaa.yaml with deploy-usb-configs.sh.
        # ════════════════════════════════════════════════════════════════════
        if path == "/autoinstall/uaa-config":
            client_ip = self.client_address[0]
            hexmac, dir_path = resolve_cloud_init_dir(client_ip)
            if not hexmac:
                log(f"UAA-CONFIG DENIED {client_ip} - no ARP/NDP neighbor entry")
                self.send_response(404)
                self.end_headers()
                return
            if not dir_path:
                log(f"UAA-CONFIG DENIED {client_ip} (hexmac={hexmac}) - no cloud-init dir registered")
                self.send_response(404)
                self.end_headers()
                return
            file_path = os.path.join(dir_path, "uaa.yaml")
            if not os.path.isfile(file_path):
                log(f"UAA-CONFIG DENIED {client_ip} (hexmac={hexmac}) - no uaa.yaml placed")
                self.send_response(404)
                self.end_headers()
                return
            body = open(file_path, "rb").read()
            log(f"UAA-CONFIG -> {client_ip} (hexmac={hexmac})")
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", len(body))
            self.end_headers()
            self.wfile.write(body)
            return

        self.send_json(404, {"error": "not found"})

    def do_POST(self):
        parsed = urlparse(self.path)
        path = parsed.path
        body = self.get_body()
        try:
            data = json.loads(body)
        except:
            self.send_json(400, {"error": "invalid json"})
            return

        # ── Register machine ──
        if path == "/api/register":
            mac = normalize_mac(data.get("mac", ""))
            hostname = data.get("hostname", "")
            ip = data.get("ip", "")
            server_type = data.get("type", "lenovo")
            if not mac or not hostname:
                self.send_json(400, {"ok": False, "error": "mac and hostname required"})
                return
            reg = load_registry()
            if mac in reg:
                log(f"REGISTER update: {mac} ({hostname})")
            else:
                log(f"REGISTER new: {mac} ({hostname}) type={server_type} - status=pending")
            reg[mac] = {
                "hostname": hostname,
                "mac": mac,
                "ip": ip,
                "type": server_type,
                "status": reg.get(mac, {}).get("status", "pending"),
                "registered_at": reg.get(mac, {}).get("registered_at", int(time.time())),
                "tpm_ek": reg.get(mac, {}).get("tpm_ek"),
            }
            save_registry(reg)
            self.send_json(200, {"ok": True, "status": reg[mac]["status"],
                "message": f"Registered. Approve with: curl http://172.16.2.30:25000/api/approve/{mac}"})
            return

        # ── Machine checkin ──
        if path == "/api/checkin":
            mac = normalize_mac(data.get("mac", ""))
            tpm_ek = data.get("tpm_ek", "")
            hostname = data.get("hostname", "")
            reg = load_registry()
            entry = reg.get(mac)
            if not entry:
                log(f"CHECKIN DENIED {mac} - not registered")
                self.send_json(403, {"ok": False, "error": "Not registered"})
                return
            if tpm_ek and not entry.get("tpm_ek"):
                entry["tpm_ek"] = tpm_ek
                log(f"CHECKIN TPM bound: {mac} ({hostname}) ek={tpm_ek[:16]}...")
            elif tpm_ek and entry.get("tpm_ek") and entry["tpm_ek"] != tpm_ek:
                log(f"CHECKIN TPM MISMATCH {mac} - expected {entry['tpm_ek'][:16]}... got {tpm_ek[:16]}...")
                self.send_json(403, {"ok": False, "error": "TPM mismatch - MAC may be spoofed"})
                return
            entry["last_seen"] = int(time.time())
            entry["last_ip"] = data.get("ip", "")
            save_registry(reg)
            log(f"CHECKIN {mac} ({hostname}) status={entry['status']}")
            self.send_json(200, {"ok": True, "status": entry["status"], "approved": entry["status"] == "approved"})
            return

        # ── Webhook (install status / log upload) ──
        if path == "/api/webhook":
            log_event(data)
            status = data.get("status", "")
            name = data.get("name", "")
            if webhook_should_flip(data):
                try:
                    ok, msg = flip_ipxe(name)
                except Exception as e:
                    ok, msg = False, f"flip failed: {e}"
                # Missing mac-<hexmac>.ipxe (USB-only host) or any flip error is
                # logged and swallowed — the webhook itself still succeeded.
                log(f"WEBHOOK {name} status={status} -> auto-flip: {msg}")
            else:
                log(f"WEBHOOK {name} event_type={data.get('event_type')} status={status}")
            for f in data.get("files", []):
                fpath = f.get("path", "unknown").replace("/", "_")
                ts = int(time.time())
                hostname = data.get("name", "unknown")
                out = os.path.join(FILES_DIR, f"{hostname}-{ts}-{os.path.basename(fpath)}")
                try:
                    content = base64.b64decode(f.get("content", ""))
                    open(out, "wb").write(content)
                    log(f"Saved log: {out}")
                except Exception as e:
                    log(f"Failed to save log {fpath}: {e}")
            self.send_json(200, {"ok": True})
            return

        if path in ("/api/finalreport", "/api/hardware-info", "/api/cloud-init"):
            log_event({"endpoint": path, **data})
            log(f"{path} from {data.get('client_id', data.get('name', '?'))}")
            self.send_json(200, {"ok": True})
            return

        # ════════════════════════════════════════════════════════════════════
        # ── YubiKey Registration ─────────────────────────────────────────
        # POST /api/yubikeys/register  body: {fingerprint, gpg_pubkey, ssh_pubkey, comment, serial}
        # ════════════════════════════════════════════════════════════════════
        if path == "/api/yubikeys/register":
            fp = data.get("fingerprint", "").upper().replace(" ", "")
            if not fp:
                self.send_json(400, {"ok": False, "error": "fingerprint required"})
                return
            gpg_pubkey = data.get("gpg_pubkey", "")
            ssh_pubkey = data.get("ssh_pubkey", "")
            comment = data.get("comment", "")
            serial = data.get("serial", "")
            yk = load_yk_registry()
            if fp in yk:
                log(f"YUBIKEY update: {fp} ({comment})")
            else:
                log(f"YUBIKEY register: {fp} ({comment}) serial={serial} - status=pending")
            yk[fp] = {
                "fingerprint": fp,
                "gpg_pubkey": gpg_pubkey,
                "ssh_pubkey": ssh_pubkey,
                "comment": comment,
                "serial": serial,
                "status": yk.get(fp, {}).get("status", "pending"),
                "registered_at": yk.get(fp, {}).get("registered_at", int(time.time())),
            }
            save_yk_registry(yk)
            approve_url = f"http://172.16.2.30:25000/api/yubikeys/approve/{fp}"
            self.send_json(200, {"ok": True, "status": yk[fp]["status"],
                "message": f"Registered. Approve with: curl {approve_url}"})
            return

        # ── Tang Server Checkin ──
        # POST /api/tang/checkin  body: {hostname, ip, mac, tang_url, adv_keys}
        if path == "/api/tang/checkin":
            hostname = data.get("hostname", "")
            ip = data.get("ip", "")
            tang_url = data.get("tang_url", f"http://{ip}")
            adv_keys = data.get("adv_keys", [])
            tang = load_tang_registry()
            tang[hostname] = {
                "hostname": hostname,
                "ip": ip,
                "tang_url": tang_url,
                "adv_keys": adv_keys,
                "last_seen": int(time.time()),
            }
            save_tang_registry(tang)
            log(f"TANG CHECKIN {hostname} ({ip}) keys={len(adv_keys)}")
            self.send_json(200, {"ok": True})
            return

        self.send_json(404, {"error": "not found"})


if __name__ == "__main__":
    reg = load_registry()
    EXISTING = {
        "6c:4b:90:bc:39:b3": {"hostname": "len-serv-001", "ip": "172.16.3.92", "type": "lenovo"},
        "6c:4b:90:bc:f8:a3": {"hostname": "len-serv-002", "ip": "172.16.3.94", "type": "lenovo"},
        "6c:4b:90:bc:f7:f4": {"hostname": "len-serv-003", "ip": "172.16.3.96", "type": "lenovo"},
    }
    for mac, info in EXISTING.items():
        if mac not in reg:
            reg[mac] = {"hostname": info["hostname"], "mac": mac, "ip": info["ip"],
                        "type": info["type"], "status": "approved",
                        "registered_at": int(time.time()), "tpm_ek": None}
    save_registry(reg)

    server = HTTPServer(("0.0.0.0", 25000), Handler)
    log("autoinstall-agent listening on :25000")
    log(f"Registry: {len(reg)} machines ({sum(1 for e in reg.values() if e['status']=='approved')} approved)")
    server.serve_forever()
