#!/usr/bin/env bash
# file: scripts/vm-gate/pkcs11-multitoken-pin-gate.sh
# version: 1.0.0
# guid: 0a5e6b38-c74d-4f21-9e80-b2153dc9a6ef
# last-edited: 2026-08-03
#
# Gate for the UAA fork of clevis-decrypt-pkcs11 (see
# docs/research/2026-08-03-clevis-pkcs11-multitoken-pin-fork.md).
#
# Run as root INSIDE a throwaway Ubuntu 26.04 guest that has clevis 23-1 and
# three SoftHSM2 tokens (nano/carriedA/carriedB, PINs 111111/222222/333333)
# provisioned by scripts/vm-gate/softhsm-setup.sh. NEVER run this on hardware.
#
# Expects, in the guest:
#   /root/clevis-decrypt-pkcs11.upstream   pristine clevis 23-1 script
#   /root/clevis-decrypt-pkcs11.uaa        the fork from clevis/ in this repo
#
# Negatives run before positives, and the header-leak check is observed RED
# (bound WITH pin-value) before it is trusted GREEN. Judge every row on
# decrypted CONTENT, never on exit code, and never via `clevis luks list`.
# UAA clevis-decrypt-pkcs11 fork gate. Negatives first. Run as root in the gate VM.
MOD=/usr/lib/softhsm/libsofthsm2.so
NANO=16cb4581ed58a6fb; CA=f17eeb4f46f9823e; CB=a84524986d43d605
TOKDIR=/var/lib/softhsm/tokens
PARK=/root/parked-tokens
PINF=/run/systemd/clevis-pkcs11.pin
LOG=/run/pk11tool.log
mkdir -p $PARK

sha() { sha256sum "$1" | cut -d' ' -f1; }

echo "##### 51.0 pkcs11-tool call-counting shim (PATH: /usr/local/bin first) #####"
cat > /usr/local/bin/pkcs11-tool <<'W'
#!/bin/bash
echo "$(date +%s.%N) pid=$$ ARGS: $*" >> /run/pk11tool.log
exec /usr/bin/pkcs11-tool "$@"
W
chmod 755 /usr/local/bin/pkcs11-tool
echo "which pkcs11-tool -> $(which pkcs11-tool)"

echo
echo "##### 51.1 map token label -> softhsm dir #####"
for l in nano carriedA carriedB; do
  for d in $TOKDIR/*/; do
    if grep -qa "$l" "$d"/token.object 2>/dev/null; then echo "$l -> $d"; echo "$d" > /root/gd-$l; fi
  done
done
cat /root/gd-nano /root/gd-carriedA /root/gd-carriedB

present() { # args = labels to keep present
  mv $PARK/* $TOKDIR/ 2>/dev/null
  for l in nano carriedA carriedB; do
    case " $* " in *" $l "*) : ;; *) mv "$(cat /root/gd-$l)" $PARK/ 2>/dev/null;; esac
  done
  echo "  present tokens: $(pkcs11-tool --module $MOD -L 2>/dev/null | grep -i 'token label' | awk -F: '{print $2}' | tr -d ' ' | tr '\n' ',')"
}

echo
echo "##### 51.2 build policies (WITH and WITHOUT pin-value) #####"
python3 - "$MOD" "$NANO" "$CA" "$CB" <<'PY'
import json,sys
M,nano,ca,cb=sys.argv[1:5]
def P(ser,tok,pin=None):
    u=f"pkcs11:serial={ser};token={tok}" + (f";pin-value={pin}" if pin else "") + f";module-path={M}"
    return {"uri":u,"mechanism":"RSA-PKCS"}
leak={"t":2,"pins":{"pkcs11":[P(nano,"nano","111111"),P(ca,"carriedA","222222"),P(cb,"carriedB","333333")]}}
safe={"t":2,"pins":{"pkcs11":[P(nano,"nano"),P(ca,"carriedA"),P(cb,"carriedB")]}}
open('/root/pol-leak.json','w').write(json.dumps(leak))
open('/root/pol-safe.json','w').write(json.dumps(safe))
PY
echo "leak: $(cat /root/pol-leak.json | cut -c1-120)..."
echo "safe: $(cat /root/pol-safe.json | cut -c1-120)..."

echo
echo "##### 51.3 P2 RED-FIRST: bind a LUKS device WITH pin-value, grep header #####"
present nano carriedA carriedB
cat > /root/hdrgrep.py <<'PY'
import base64,re,subprocess,sys
img=sys.argv[1]; needles=sys.argv[2:]
txt=subprocess.run(["cryptsetup","luksDump","--dump-json-metadata",img],
                   capture_output=True,text=True).stdout
blob=txt
for _ in range(5):
    add=[]
    for m in set(re.findall(r'[A-Za-z0-9_-]{40,}', blob)):
        try: add.append(base64.urlsafe_b64decode(m+'='*(-len(m)%4)).decode('utf-8','replace'))
        except Exception: pass
    blob = blob + "\n" + "\n".join(add)
print("decoded_blob_bytes=%d" % len(blob))
for n in needles:
    b64=base64.urlsafe_b64encode(n.encode()).decode().rstrip('=')
    print("  needle %-12s plaintext_in_header=%-5s b64_in_header=%-5s"
          % (repr(n), n in blob, b64 in blob))
PY
for kind in leak safe; do
  IMG=/root/hdr-$kind.img
  rm -f $IMG; truncate -s 64M $IMG
  printf 'gatepass' > /root/hp.txt
  cryptsetup luksFormat --type luks2 --batch-mode --pbkdf pbkdf2 --pbkdf-force-iterations 1000 $IMG /root/hp.txt 2>/dev/null
  clevis luks bind -y -d $IMG -k /root/hp.txt sss "$(cat /root/pol-$kind.json)" </dev/null >/dev/null 2>/root/bind-$kind.err
  echo "  bind($kind) rc=$? $(head -1 /root/bind-$kind.err)"
  echo "  --- header grep for $kind ---"
  python3 /root/hdrgrep.py $IMG 111111 222222 333333 pin-value
done

###### part 2: rows ######
MOD=/usr/lib/softhsm/libsofthsm2.so
NANO=16cb4581ed58a6fb; CA=f17eeb4f46f9823e; CB=a84524986d43d605
TOKDIR=/var/lib/softhsm/tokens; PARK=/root/parked-tokens
PINF=/run/systemd/clevis-pkcs11.pin; LOG=/run/pk11tool.log
id_for(){ printf '%s|%s' "$1" "$2" | sha256sum | cut -c1-32; }
IDN=$(id_for $NANO nano); IDA=$(id_for $CA carriedA); IDB=$(id_for $CB carriedB)
echo "per-token PIN files: nano=$PINF.$IDN carriedA=$PINF.$IDA carriedB=$PINF.$IDB"

present() { mv $PARK/* $TOKDIR/ 2>/dev/null
  for l in nano carriedA carriedB; do case " $* " in *" $l "*) :;; *) mv "$(cat /root/gd-$l)" $PARK/ 2>/dev/null;; esac; done
  echo "  present: $(/usr/bin/pkcs11-tool --module $MOD -L 2>/dev/null | grep -i 'token label' | awk -F: '{print $2}' | tr -d ' ' | tr '\n' ',')"; }

echo "##### 52.0 encrypt marker under the pin-value-FREE 2-of-3 policy #####"
present nano carriedA carriedB
printf 'MARKER-52-OK' | clevis encrypt sss "$(cat /root/pol-safe.json)" > /root/safe.jwe 2>/root/e.err
echo "  encrypt_rc=$? bytes=$(stat -c%s /root/safe.jwe)"; head -2 /root/e.err

run_row(){ # name  expect  "present labels"  script-path
  local n="$1" e="$2" p="$3" s="$4"
  echo "=================================================================="
  echo "ROW $n   EXPECT=$e   script=$(basename $s)"
  rm -f $PINF $PINF.* ; : > $LOG
  present $p
  eval "$SEED"
  cp -a "$s" /usr/bin/clevis-decrypt-pkcs11; chmod 755 /usr/bin/clevis-decrypt-pkcs11
  out=$(timeout 120 clevis decrypt < /root/safe.jwe 2>/root/d.err); rc=$?
  local logins=$(grep -c -- '--login' $LOG)
  echo "  rc=$rc  OUT=[$out]  content_match=$([ "$out" = "MARKER-52-OK" ] && echo YES || echo NO)"
  echo "  pkcs11-tool --login invocations: $logins"
  echo "  stderr:"; grep -aiE 'PIN|slot|not present|invalid option|exhausted|Unable' /root/d.err | sort -u | head -8 | sed 's/^/    /'
}

FORK=/root/clevis-decrypt-pkcs11.uaa
UP=/root/clevis-decrypt-pkcs11.upstream

SEED=':'
run_row "C0 CONFOUND: upstream, 2 tokens, shared one-shot PIN file" "MUSTFAIL" "carriedA carriedB" $UP
echo "  (seeding shared file the upstream way and retrying)"
rm -f $PINF $PINF.*; : > $LOG; echo -n '222222' > $PINF
out=$(timeout 120 clevis decrypt < /root/safe.jwe 2>/root/d.err); echo "  upstream+sharedfile rc=$? OUT=[$out]"
grep -aiE 'invalid option|Invalid PIN|Unable' /root/d.err | sort -u | head -4 | sed 's/^/    /'

SEED=':'
run_row "N1 fork, ONE token present" "MUSTFAIL" "carriedA" $FORK

SEED="( umask 077; echo -n 222222 > $PINF.$IDA; echo -n WRONGPIN > $PINF.$IDB )"
run_row "N2 fork, two tokens, WRONG pin on carriedB" "MUSTFAIL" "carriedA carriedB" $FORK

SEED="( umask 077; echo -n 222222 > $PINF.$IDA; echo -n 333333 > $PINF.$IDB )"
run_row "P1 fork, two tokens, two DIFFERENT correct PINs" "MUSTPASS" "carriedA carriedB" $FORK

echo "=================================================================="
echo "ROW P3 fork, interactive systemd-ask-password, no seeded PINs   EXPECT=MUSTPASS"
rm -f $PINF $PINF.*; : > $LOG; : > /run/askagent.log
present carriedA carriedB
cp -a $FORK /usr/bin/clevis-decrypt-pkcs11; chmod 755 /usr/bin/clevis-decrypt-pkcs11
cat > /root/agent.py <<'PY'
import os,socket,time,glob,configparser,sys
ans={'carriedA':'222222','carriedB':'333333'}
seen=set(); end=time.time()+90
log=open('/run/askagent.log','a',buffering=1)
while time.time()<end:
    for f in glob.glob('/run/systemd/ask-password/ask.*'):
        if f in seen: continue
        try:
            c=configparser.ConfigParser(); c.read(f)
            msg=c['Ask']['Message']; sock=c['Ask']['Socket']
        except Exception: continue
        seen.add(f)
        pw=next((v for k,v in ans.items() if k in msg), None)
        log.write("PROMPT t=%.3f msg=%r -> %s\n"%(time.time(),msg,'ANSWERED' if pw else 'NO-MATCH'))
        if pw:
            s=socket.socket(socket.AF_UNIX,socket.SOCK_DGRAM)
            s.bind('/run/systemd/agent-%d-%d'%(os.getpid(),len(seen)))
            s.sendto(b'+'+pw.encode()+b'\0',sock); s.close()
    time.sleep(0.1)
PY
rm -f /run/systemd/agent-* ; python3 /root/agent.py & AG=$!
sleep 1
out=$(timeout 120 clevis decrypt < /root/safe.jwe 2>/root/d.err); rc=$?
kill $AG 2>/dev/null; rm -f /run/systemd/agent-*
echo "  rc=$rc OUT=[$out] content_match=$([ "$out" = "MARKER-52-OK" ] && echo YES || echo NO)"
echo "  --- prompts observed ---"; sed 's/^/    /' /run/askagent.log
echo "  pkcs11-tool --login invocations: $(grep -c -- '--login' $LOG)"
grep -aiE 'PIN|not present' /root/d.err | sort -u | head -5 | sed 's/^/    /'

echo
echo "##### restore all tokens #####"; present nano carriedA carriedB
