# Intel AMT Setup for M715q Watchdog Recovery

## Overview

The Lenovo ThinkCentre M715q uses Intel AMT (Active Management Technology) with DASH 1.0/1.1 protocol for out-of-band power management. This replaces traditional IPMI for remote power control fallback in the Tang server watchdog recovery strategy.

**Status**: DASH Support is already enabled in BIOS on len-serv-001 and len-serv-002.

## Architecture

### Network Access
Intel AMT is accessed via WSMAN (Web Services Management) over the network, not through local device files. The M715q can be accessed via:
1. **Dedicated Management IP** (if configured in BIOS)
2. **Shared NIC** (same as OS network)

### Access Methods
- **Primary**: wsmancli (WSMAN command-line client)
- **Alternative**: Direct HTTPS to Intel AMT web console (usually port 16992)
- **Alternative**: iLO-style tools if available

## Required Components

### 1. Install WSMAN Client
```bash
# On management station (len-serv-001 or external control host)
sudo apt-get install wsmancli
```

### 2. Configure Intel AMT Credentials

Intel AMT requires separate credentials from the OS. Default configuration:
- **Username**: admin
- **Password**: Set during BIOS setup or first-time access

#### Setting Credentials via BIOS (Lenovo ThinkLMI)
On the M715q system itself:
```bash
# Access via ThinkLMI sysfs (if available)
# or through BIOS Setup (Del/F1 during boot)
# Set: Intellilligent Thermaling Management > Settings
#      Admin Password (for Intel AMT)
```

**Alternative**: If no AMT password is set, Intel AMT may use default credentials or allow first-time setup.

### 3. Discover AMT Interface Address
```bash
# From within M715q OS:
nmcli device show | grep IP

# Or from BIOS settings:
# System Settings > Network > IPv4 Address (for dedicated AMT NIC if enabled)
```

For most Lenovo systems with shared NIC, use the system's main IP address.

## Remote Power Control via WSMAN

### Power State Management
```bash
# Power off (graceful)
wsmancli -U <username> -P <password> -h <ip_address> \
  "http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ComputerSystem" \
  call RequestStateChange RequestedState=4

# Power on
wsmancli -U <username> -P <password> -h <ip_address> \
  "http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ComputerSystem" \
  call RequestStateChange RequestedState=2

# Hard power off (immediate)
wsmancli -U <username> -P <password> -h <ip_address> \
  "http://schemas.intel.com/wbem/wscim/1/amt-schema/1/AMT_Boot" \
  call RequestPowerStateChange PowerState=8 Force=1
```

### State Values
- **2**: Power On
- **4**: Power Off (graceful)
- **8**: Hard Off (immediate)
- **10**: Reboot

### Alternative: Direct Power Cycle Script
For watchdog fallback, a simple power cycle:
```bash
#!/bin/bash
# m715q-power-cycle.sh
AMT_IP="<target-ip>"
AMT_USER="admin"
AMT_PASS="<password>"

# Hard reset via Intel AMT
wsmancli -U "$AMT_USER" -P "$AMT_PASS" -h "$AMT_IP" \
  "http://schemas.intel.com/wbem/wscim/1/amt-schema/1/AMT_Boot" \
  call RequestPowerStateChange PowerState=8 Force=1

# Wait for reboot
sleep 10

# Power on
wsmancli -U "$AMT_USER" -P "$AMT_PASS" -h "$AMT_IP" \
  "http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ComputerSystem" \
  call RequestStateChange RequestedState=2
```

## Integration with Tang Watchdog Recovery

### Watchdog Fallback Strategy (M715q / len-serv-001-3)

1. **systemd watchdog** (primary): 180-second timeout
2. **Software watchdog daemon** (secondary): Feeds /dev/watchdog
3. **Intel AMT remote reset** (fallback): Triggered if software watchdog fails

### Systemd Service for AMT Power Reset
```ini
[Unit]
Description=Tang Server Intel AMT Watchdog Fallback
Wants=network-online.target
After=network-online.target
ConditionPathExists=/sys/class/firmware-attributes/thinklmi

[Service]
Type=simple
ExecStart=/usr/local/bin/amt-watchdog-fallback.sh
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

## Security Considerations

1. **Credentials Storage**: Store AMT credentials in restricted file (mode 600) or systemd credential store
2. **Network Access**: Restrict WSMAN port (16992) to trusted management network
3. **Default Credentials**: Change default admin password immediately after setup
4. **Audit Logging**: Enable Intel AMT audit logging in BIOS for remote power operations

## Testing Checklist

- [ ] Verify DASH Support is enabled in BIOS: `cat /sys/class/firmware-attributes/thinklmi/attributes/'DASH Support'/current_value`
- [ ] Verify Onboard Ethernet is enabled: `cat /sys/class/firmware-attributes/thinklmi/attributes/'Onboard Ethernet Controller'/current_value`
- [ ] Install wsmancli: `sudo apt-get install wsmancli`
- [ ] Test remote power queries: `wsmancli -U admin -P <pass> -h <ip> get`
- [ ] Test power state change: `wsmancli ... call RequestStateChange RequestedState=4`
- [ ] Verify system powers off gracefully
- [ ] Test hard reset: `wsmancli ... call RequestPowerStateChange PowerState=8 Force=1`
- [ ] Verify automatic power-on after hard reset

## Future Enhancements

- [ ] Integrate with centralized monitoring dashboard (freeipmi/IPMI or similar)
- [ ] Add event logging for remote power operations
- [ ] Implement health checks before triggering power reset
- [ ] Create management API for coordinated M715q/RPi recovery
