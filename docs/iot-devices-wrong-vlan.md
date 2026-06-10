# IoT Devices on Wrong Network (VentureIndustries → BillyQuizBoy)

Devices identified on the **VentureIndustries** network (`172.16.3.0/23`) that should be on the **BillyQuizBoy** IoT VLAN (`172.16.0.0/23`).

Identified via ARP scan from `len-serv-001` (`172.16.3.92`) on 2026-05-24 and cross-referenced with UniFi client list.

## Confirmed misplaced (definitely IoT)

These devices are clearly consumer IoT and should be on BillyQuizBoy alongside their siblings.

### Nest Thermostat
- **UniFi name**: `Nest-Thermostat-FFF5`
- **MAC**: `3c:31:74:b7:ff:f5`
- **Vendor (OUI)**: Google / Nest (`3C:31:74`)
- **Current IP**: `172.16.3.79`
- **Connection**: WiFi 4, 1×1, 5 GHz ch 40 (via Access Point U7 Pro Max)
- **Lease type**: Dynamic
- **Why this is IoT**: Nest devices live on BillyQuizBoy alongside Nest-3663, Nest-39C4, Nest-Hello-e597, Nest-Connect-6936
- **Remediation**: In the Nest app, forget WiFi, rejoin the BillyQuizBoy SSID

### Meross Smart Plug
- **UniFi name**: `Meross Smart Plug aa:31`
- **MAC**: `48:e1:e9:23:aa:31`
- **Vendor (OUI)**: Chengdu Meross Technology (`48:E1:E9`)
- **Current IP**: `172.16.3.102`
- **Connection**: WiFi 4, 1×1, 2.4 GHz ch 1 (via U6-LR Dining Room)
- **Lease type**: Dynamic
- **Why this is IoT**: Meross smart plugs/switches all live on BillyQuizBoy (Meross Smart Plug 98:8b, 7a:c0, b0:2e, Meross Smart Switch 13:32)
- **Remediation**: Factory reset the plug, re-onboard via Meross app onto BillyQuizBoy SSID

### TP-Link Kasa Smart Switch
- **UniFi name**: `KS240`
- **MAC**: `40:ae:30:a5:c0:87`
- **Vendor (OUI)**: TP-Link / Kasa (`40:AE:30`)
- **Current IP**: `172.16.3.121`
- **Connection**: WiFi 4, 1×1, 2.4 GHz ch 11 (via U6-LR Front Door)
- **Lease type**: Dynamic
- **Why this is IoT**: KS240 is a Kasa 2-gang smart switch — sibling devices (HS210 33:ea on BillyQuizBoy at `172.16.1.194`)
- **Remediation**: Use Kasa app → device settings → Wi-Fi → switch to BillyQuizBoy SSID

## Possibly misplaced (judgment call)

These are arguably IoT but could legitimately live on the main network depending on policy. Listed for review.

### HDHomeRun TV Tuner
- **UniFi name**: `HDHR-10A88A7D`
- **MAC**: `00:18:dd:0a:88:a7`
- **Vendor (OUI)**: SiliconDust / HDHomeRun (`00:18:DD`)
- **Current IP**: `172.16.3.26`
- **Connection**: Wired (USW-Pro-24 Port 8), FE
- **Lease type**: **Fixed** (deliberately pinned)
- **Note**: Wired device, intentional reservation — leave unless you want IoT isolated from VentureIndustries clients that consume the tuner stream

### Prusa Core One (3D Printer)
- **UniFi name**: `prusa-core-one`
- **MAC**: `10:9c:70:29:7d:9b`
- **Vendor (OUI)**: Prusa Research (`10:9C:70`)
- **Current IP**: `172.16.3.148`
- **Connection**: Wired (Family Room Switch Lite 8 PoE Port 6), FE
- **Lease type**: Dynamic
- **Note**: Wired, accessed by OctoPrint hosts and laptops — moving to IoT VLAN may complicate access from main network without firewall rules

### Canon Printer
- **UniFi name**: `Canon22e51a`
- **MAC**: `6c:f2:d8:22:e5:1a`
- **Vendor (OUI)**: Canon (`6C:F2:D8`)
- **Current IP**: `172.16.3.181`
- **Connection**: Wired (Switch Pro Max 16 PoE Port 5), GbE
- **Lease type**: Fixed
- **Note**: Wired printer — same trade-off as Prusa, needs reachability from print clients

## Why this matters

- BillyQuizBoy is the **IoT isolation VLAN** — devices there are restricted from talking to management/main network resources (firewall policy).
- Devices that ended up on VentureIndustries got there because they were onboarded on the wrong SSID at setup time, **not** because of a policy decision.
- Leaving them on VentureIndustries means an unaudited IoT device (often with weak firmware, default creds, cloud telephony) sits on the same broadcast domain as `len-serv-001/2/3`, your MacBook, and other trust-tier-1 hosts.

## How to move a device (WiFi)

VLAN membership for WiFi clients is determined by which SSID they join. To move:

1. On the device, **forget the current WiFi network** (VentureIndustries)
2. Connect to **BillyQuizBoy** SSID instead
3. UniFi will fingerprint it on its first DHCP request on the new VLAN and assign a `172.16.0.x` or `172.16.1.x` lease
4. Optionally, set a fixed-IP reservation in UniFi for stability

For Meross / Kasa / Nest devices, this usually means using their respective apps' "change WiFi network" flow or factory-resetting and re-onboarding.

## Reference: Identifying information format

When adding new devices found later:
```
- UniFi name: <name as shown in UniFi>
- MAC: <full lowercase colon-separated MAC>
- Vendor (OUI): <vendor name> (<first 3 octets>)
- Current IP: <ip>
- Connection: <wired or WiFi details>
- Lease type: <Fixed or Dynamic>
- Why this is IoT: <reasoning + siblings on correct VLAN>
- Remediation: <specific steps>
```
