// file: crates/uaa-core/src/network/ssh_installer/packages.rs
// version: 2.3.0
// guid: sshpkg01-2345-6789-abcd-ef0123456789
// last-edited: 2026-08-02

//! Package management for SSH installation
//!
//! Also owns the (opt-in, off-by-default) apt plumbing that pulls **clevis 23**
//! from the Ubuntu 26.10 pocket onto a 26.04 host so the clevis **pkcs11 pin**
//! (YubiKey PIV as a LUKS unlock factor) becomes available. See
//! `docs/research/2026-08-02-clevis23-pkcs11-pinning-risk.md` for the measured
//! facts and the one unresolved dependency collision.

use crate::network::CommandExecutor;
use crate::Result;
use tracing::info;

/// Base package set installed on the **live** host before the install runs.
///
/// Kept as a named const so the default-off path is provably byte-identical to
/// the pre-clevis-23 behaviour (see `default_off_package_list_is_unchanged`).
///
/// NOTE (live environment, not the target): clevis + clevis-luks are required
/// HERE because `clevis luks bind` for Tang enrollment runs on the live host
/// against the LUKS partition (the mapper is not visible in the chroot). The
/// 26.04 live-server ISO does NOT ship clevis, so without this Tang enrollment
/// silently skips. mdadm is needed to assemble/query IMSM (BIOS fake-RAID)
/// volumes like unimatrixone's /dev/md126; harmless on hosts without md devices.
/// clevis-tpm2 + tpm2-tools back the clevis SSS *tpm2 pin*: `clevis luks bind
/// sss` runs `clevis-encrypt-tpm2` on the LIVE host, and without them the tpm2
/// share of the D2-B binding silently never gets created.
pub const LIVE_BASE_PACKAGES: &[&str] = &[
    "cryptsetup",
    "parted",
    "gdisk",
    "debootstrap",
    "dosfstools",
    "xfsprogs",
    "util-linux",
    "clevis",
    "clevis-luks",
    "clevis-tpm2",
    "tpm2-tools",
    "mdadm",
];

/// Suite (Ubuntu 26.10 "Stonking Stingray") that ships `clevis 23-1build1`,
/// the first clevis with a pkcs11 pin. 26.04 "resolute" only has `20-1ubuntu2`
/// (universe), and `resolute-backports` does NOT carry clevis.
pub const CLEVIS23_SUITE: &str = "stonking";

/// Mirror the clevis-23 pocket is fetched from. Same archive as the base
/// system, only a different suite.
pub const CLEVIS23_MIRROR: &str = "http://archive.ubuntu.com/ubuntu/";

/// deb822 source file dropped when the clevis-23 path is enabled.
pub const CLEVIS23_SOURCES_PATH: &str = "/etc/apt/sources.list.d/uaa-clevis23.sources";

/// apt pin file dropped alongside it. `99-` so it wins over anything a base
/// image ships.
pub const CLEVIS23_PREFERENCES_PATH: &str = "/etc/apt/preferences.d/99-uaa-clevis23-pkcs11";

/// The ONLY packages allowed to come from the 26.10 pocket.
///
/// The clevis binary packages are lockstep-versioned (`clevis-luks Depends:
/// clevis (= 23-1build1)`, `clevis-systemd Depends: clevis-luks (= …)`,
/// `clevis-dracut Depends: clevis-systemd (= …)`, `clevis-tpm2 Depends: clevis
/// (= …)`), so the pin has to cover the whole set or apt cannot resolve it.
/// There is NO separate `clevis-pkcs11` package — clevis 23's main binary
/// package ships `clevis-encrypt-pkcs11`, `clevis-decrypt-pkcs11`,
/// `clevis-pkcs11-afunix-socket-unlock` and `clevis-pkcs11-common`; the dracut
/// module `50clevis-pin-pkcs11` is in `clevis-dracut` and
/// `clevis-luks-pkcs11-askpass.service` is in `clevis-systemd`.
///
/// `libssl4` is here because clevis 23 `Depends: libssl4 (>= 4.0.0)`, which
/// does not exist in 26.04. It co-installs with the installed `libssl3t64`
/// (distinct sonames `libssl.so.4`/`libcrypto.so.4`), so there is no glibc
/// cascade. `openssl-provider-legacy` is here because the 26.10 `libssl4
/// 4.0.1-1ubuntu1 Depends: openssl-provider-legacy (>= 4.0.0)` and that is a
/// single package name — see the risk doc; this is the unresolved edge.
pub const CLEVIS23_PINNED_PACKAGES: &[&str] = &[
    "clevis",
    "clevis-luks",
    "clevis-dracut",
    "clevis-systemd",
    "clevis-tpm2",
    "libssl4",
    "openssl-provider-legacy",
];

/// PKCS#11 / PIV token userspace. `clevis` only *Recommends* opensc, and
/// nothing pulls `pcscd`, so without these the pkcs11 pin has no token to talk
/// to. Deliberately NOT in `CLEVIS23_PINNED_PACKAGES`: these install from plain
/// 26.04 universe at their stock versions.
pub const PKCS11_TOKEN_PACKAGES: &[&str] = &["opensc", "pcscd"];

/// Render the deb822 source for the clevis-23 pocket.
///
/// `Components: main universe` is load-bearing — clevis lives in *universe*,
/// while `libssl4` / `openssl-provider-legacy` live in *main*. Dropping either
/// component makes the whole thing a silent no-op that resolves clevis 20.
pub fn clevis23_sources_content(suite: &str, mirror: &str) -> String {
    format!(
        "# Managed by ubuntu-autoinstall-agent. Enabled by clevis_pkcs11_pin.\n\
         # Sole purpose: make clevis 23 (the first clevis with a pkcs11 pin)\n\
         # installable on a 26.04 host. Scope is clamped by\n\
         # {prefs}, which pins everything else in this suite to -1.\n\
         Types: deb\n\
         URIs: {mirror}\n\
         Suites: {suite}\n\
         Components: main universe\n\
         Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n",
        prefs = CLEVIS23_PREFERENCES_PATH,
        mirror = mirror,
        suite = suite,
    )
}

/// Render the apt pin that clamps the clevis-23 pocket to exactly
/// [`CLEVIS23_PINNED_PACKAGES`].
///
/// Two blocks, and BOTH are required:
///
/// 1. The allowlist at priority 501 — above the default 500 so these specific
///    packages are taken from the newer suite.
/// 2. A `Package: *` catch-all for the same release at -1 (never install).
///    Without it the pocket sits at the default 500 and, because every 26.10
///    package is higher-versioned than its 26.04 counterpart, apt would happily
///    dist-upgrade the base system out from under us on the next
///    `apt full-upgrade`. The whole point of this file is that it is narrow.
pub fn clevis23_preferences_content(suite: &str) -> String {
    format!(
        "# Managed by ubuntu-autoinstall-agent. Enabled by clevis_pkcs11_pin.\n\
         #\n\
         # WHY THIS FILE IS DELIBERATELY NARROW:\n\
         # The {suite} pocket is added ONLY to obtain clevis 23, which is the\n\
         # first clevis release carrying the pkcs11 pin (YubiKey PIV as a LUKS\n\
         # unlock factor). 26.04 ships clevis 20, which has no such pin, and it\n\
         # is not in resolute-backports either. A broad pin here would silently\n\
         # upgrade the entire base system to 26.10 on the next full-upgrade,\n\
         # because every package in the newer suite outranks its 26.04 twin by\n\
         # version. So: an explicit allowlist above default priority, and a\n\
         # catch-all below zero for everything else in the same release.\n\
         #\n\
         # The clevis binaries are lockstep-versioned (clevis-luks Depends:\n\
         # clevis (= 23-1build1), and so on down the chain), so the allowlist\n\
         # must name the whole set - pinning one package alone is unresolvable.\n\
         # libssl4 is a clevis 23 dependency absent from 26.04; it co-installs\n\
         # with libssl3t64 because the sonames differ. openssl-provider-legacy\n\
         # is a libssl4 dependency and is NOT co-installable - see\n\
         # docs/research/2026-08-02-clevis23-pkcs11-pinning-risk.md.\n\
         #\n\
         # opensc / pcscd are intentionally ABSENT: they install from plain\n\
         # 26.04 universe at their stock versions.\n\
         \n\
         Package: {allow}\n\
         Pin: release n={suite}\n\
         Pin-Priority: 501\n\
         \n\
         Package: *\n\
         Pin: release n={suite}\n\
         Pin-Priority: -1\n",
        suite = suite,
        allow = CLEVIS23_PINNED_PACKAGES.join(" "),
    )
}

/// Shell commands that lay down the clevis-23 apt plumbing under `root`.
///
/// `root` is `""` for the live host and `"/mnt/targetos"` for the target
/// chroot's filesystem when writing from outside it. The heredocs use the
/// `<<\EOF` form (backslash-escaped delimiter): it suppresses expansion exactly
/// like `<<'EOF'` but contains no single quote, so these commands survive being
/// wrapped in `bash -lc '…'` — which is how every chroot command in this
/// installer is run.
///
/// ORDERING IS LOAD-BEARING: these must run BEFORE `apt update`, or the 26.10
/// pocket is never indexed and the install silently resolves clevis 20.
pub fn clevis23_apt_config_commands(root: &str) -> Vec<String> {
    let sources = format!("{root}{CLEVIS23_SOURCES_PATH}");
    let prefs = format!("{root}{CLEVIS23_PREFERENCES_PATH}");
    vec![
        format!(
            "mkdir -p {root}/etc/apt/sources.list.d {root}/etc/apt/preferences.d",
            root = root
        ),
        format!(
            "cat > {sources} <<\\EOF\n{body}EOF",
            sources = sources,
            body = clevis23_sources_content(CLEVIS23_SUITE, CLEVIS23_MIRROR)
        ),
        format!(
            "cat > {prefs} <<\\EOF\n{body}EOF",
            prefs = prefs,
            body = clevis23_preferences_content(CLEVIS23_SUITE)
        ),
    ]
}

/// The live-host package list, given whether the pkcs11 pin path is enabled.
///
/// Default (`false`) returns exactly [`LIVE_BASE_PACKAGES`] — byte-identical to
/// the pre-clevis-23 behaviour, which is what keeps len-serv-001/002
/// unaffected.
pub fn live_packages(clevis_pkcs11_pin: bool) -> Vec<&'static str> {
    let mut pkgs: Vec<&'static str> = LIVE_BASE_PACKAGES.to_vec();
    if clevis_pkcs11_pin {
        pkgs.extend_from_slice(PKCS11_TOKEN_PACKAGES);
    }
    pkgs
}

/// Extra packages appended to the **target chroot** apt line when the pkcs11
/// pin is enabled, as a pre-spaced suffix (the existing chroot install lines are
/// built by string concatenation).
///
/// The clevis pkcs11 *pin* itself needs no extra package — clevis 23's main
/// binary package carries it, `clevis-dracut` carries the
/// `50clevis-pin-pkcs11` dracut module and `clevis-systemd` carries
/// `clevis-luks-pkcs11-askpass.service`, and all three are already on the
/// target's install line. What is missing is the PIV token userspace.
///
/// Returns `""` when off, so the default path is byte-identical.
pub fn target_pkcs11_package_suffix(clevis_pkcs11_pin: bool) -> &'static str {
    if clevis_pkcs11_pin {
        " opensc pcscd"
    } else {
        ""
    }
}

pub struct PackageManager<'a> {
    runner: &'a mut dyn CommandExecutor,
}

impl<'a> PackageManager<'a> {
    pub fn new(runner: &'a mut dyn CommandExecutor) -> Self {
        Self { runner }
    }

    /// Install required packages for installation.
    ///
    /// `clevis_pkcs11_pin` opts this host into clevis 23 from the 26.10 pocket
    /// (see module docs). Off by default; when off, not a single command
    /// changes.
    pub async fn install_required_packages(&mut self, clevis_pkcs11_pin: bool) -> Result<()> {
        info!("Installing required packages (clevis_pkcs11_pin={clevis_pkcs11_pin})");

        // Apt plumbing FIRST — an `apt-get update` that predates the new source
        // would leave the 26.10 pocket unindexed and clevis 20 would win.
        if clevis_pkcs11_pin {
            info!("clevis pkcs11 pin enabled: adding pinned {CLEVIS23_SUITE} pocket");
            for cmd in clevis23_apt_config_commands("") {
                self.runner.execute(&cmd).await?;
            }
        }

        // Update package lists first
        self.runner.execute("apt-get update").await?;

        // Install ZFS utilities specifically
        self.runner
            .execute("DEBIAN_FRONTEND=noninteractive apt-get install -y zfsutils-linux")
            .await?;

        let install_cmd = format!(
            "DEBIAN_FRONTEND=noninteractive apt-get install -y {}",
            live_packages(clevis_pkcs11_pin).join(" ")
        );
        self.runner.execute(&install_cmd).await?;

        info!("Required packages installed successfully");
        Ok(())
    }

    /// Check if specific tools are available
    pub async fn check_tool_availability(&mut self, tools: &[&str]) -> Result<Vec<String>> {
        let mut available = Vec::new();

        for tool in tools {
            match self
                .runner
                .execute(&format!("command -v {} >/dev/null 2>&1", tool))
                .await
            {
                Ok(_) => available.push(tool.to_string()),
                Err(_) => continue,
            }
        }

        Ok(available)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every package name the pin file's `Package:` line names, as exact
    /// tokens. `contains()` is useless here — "clevis" is a substring of
    /// "clevis-luks" — so match whole whitespace-separated tokens.
    fn pinned_tokens(content: &str) -> Vec<&str> {
        content
            .lines()
            .filter(|l| l.starts_with("Package: ") && !l.starts_with("Package: *"))
            .flat_map(|l| l.trim_start_matches("Package: ").split_whitespace())
            .collect()
    }

    #[test]
    fn default_off_package_list_is_unchanged() {
        // The exact set installed on the live host before clevis-23 support
        // existed. If this ever drifts, len-serv-001/002 are no longer getting
        // what they got on their last known-good install.
        let expected = vec![
            "cryptsetup",
            "parted",
            "gdisk",
            "debootstrap",
            "dosfstools",
            "xfsprogs",
            "util-linux",
            "clevis",
            "clevis-luks",
            "clevis-tpm2",
            "tpm2-tools",
            "mdadm",
        ];
        assert_eq!(live_packages(false), expected);
        assert_eq!(
            format!(
                "DEBIAN_FRONTEND=noninteractive apt-get install -y {}",
                live_packages(false).join(" ")
            ),
            "DEBIAN_FRONTEND=noninteractive apt-get install -y cryptsetup parted gdisk \
             debootstrap dosfstools xfsprogs util-linux clevis clevis-luks clevis-tpm2 \
             tpm2-tools mdadm"
        );
    }

    #[test]
    fn default_off_emits_no_apt_plumbing() {
        // The default-off path must not merely write an empty file — the
        // commands must not exist at all.
        assert!(!live_packages(false).contains(&"opensc"));
        assert!(!live_packages(false).contains(&"pcscd"));
    }

    #[test]
    fn enabled_adds_only_token_packages_to_the_live_set() {
        let on = live_packages(true);
        let off = live_packages(false);
        assert_eq!(&on[..off.len()], &off[..], "base set must be a prefix");
        assert_eq!(&on[off.len()..], PKCS11_TOKEN_PACKAGES);
    }

    #[test]
    fn sources_content_names_suite_components_and_keyring() {
        let s = clevis23_sources_content("stonking", "http://archive.ubuntu.com/ubuntu/");
        assert!(s.contains("Suites: stonking"), "{s}");
        // universe (clevis) AND main (libssl4) are both required.
        assert!(s.contains("Components: main universe"), "{s}");
        assert!(
            s.contains("Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg"),
            "deb822 without Signed-By fails apt update outright: {s}"
        );
        assert!(s.contains("Types: deb"), "{s}");
        // Must survive `bash -lc '…'` wrapping.
        assert!(!s.contains('\''), "no single quotes in generated content");
    }

    #[test]
    fn preferences_pin_the_whole_lockstep_clevis_set() {
        let p = clevis23_preferences_content("stonking");
        let tokens = pinned_tokens(&p);
        assert_eq!(tokens, CLEVIS23_PINNED_PACKAGES.to_vec());
        for pkg in [
            "clevis",
            "clevis-luks",
            "clevis-dracut",
            "clevis-systemd",
            "clevis-tpm2",
        ] {
            assert!(
                tokens.contains(&pkg),
                "lockstep-versioned {pkg} missing from pin: {p}"
            );
        }
        assert!(p.contains("Pin: release n=stonking"), "{p}");
        assert!(p.contains("Pin-Priority: 501"), "{p}");
    }

    #[test]
    fn preferences_do_not_match_packages_outside_the_intended_set() {
        let p = clevis23_preferences_content("stonking");
        let tokens = pinned_tokens(&p);
        // opensc/pcscd come from plain 26.04 universe — pinning them to the
        // 26.10 pocket is the easiest mistake to make while adding both in one
        // commit. Everything else here is base system that must NEVER be
        // sourced from 26.10.
        for forbidden in [
            "opensc",
            "pcscd",
            "libc6",
            "systemd",
            "zfsutils-linux",
            "dracut-core",
            "dracut-network",
            "tpm2-tools",
            "openssh-server",
            "linux-image-generic",
            "grub-efi-amd64",
            "libssl3t64",
            "*",
        ] {
            assert!(
                !tokens.contains(&forbidden),
                "{forbidden} must not be pinned to the 26.10 pocket: {p}"
            );
        }
    }

    #[test]
    fn preferences_include_the_catch_all_deny_block() {
        let p = clevis23_preferences_content("stonking");
        // Without this the pocket sits at the default 500 and the base system
        // becomes dist-upgradable to 26.10.
        assert!(p.contains("Package: *"), "{p}");
        assert!(p.contains("Pin-Priority: -1"), "{p}");
        let star = p.find("Package: *").expect("catch-all block");
        assert!(
            p[star..].contains("Pin: release n=stonking"),
            "catch-all must be scoped to the added suite, not global: {p}"
        );
    }

    #[test]
    fn preferences_explain_why_the_pin_is_narrow() {
        let p = clevis23_preferences_content("stonking");
        assert!(p.contains("WHY THIS FILE IS DELIBERATELY NARROW"), "{p}");
        assert!(!p.contains('\''), "no single quotes in generated content");
    }

    #[test]
    fn apt_config_commands_write_both_files_under_root() {
        let cmds = clevis23_apt_config_commands("/mnt/targetos");
        assert_eq!(cmds.len(), 3);
        assert!(cmds[0].contains("mkdir -p /mnt/targetos/etc/apt/sources.list.d"));
        assert!(cmds[1].contains("cat > /mnt/targetos/etc/apt/sources.list.d/uaa-clevis23.sources"));
        assert!(
            cmds[2].contains("cat > /mnt/targetos/etc/apt/preferences.d/99-uaa-clevis23-pkcs11")
        );
        for c in &cmds {
            // These get wrapped in `chroot … bash -lc '<cmd>'`.
            assert!(!c.contains('\''), "single quote would break bash -lc: {c}");
        }
        // `<<\EOF` (not `<<'EOF'`) for exactly that reason.
        assert!(cmds[1].contains("<<\\EOF"), "{}", cmds[1]);
        assert!(cmds[1].ends_with("\nEOF"), "{}", cmds[1]);
    }

    #[test]
    fn apt_config_commands_write_absolute_paths_on_the_live_host() {
        let cmds = clevis23_apt_config_commands("");
        assert!(cmds[1].contains("cat > /etc/apt/sources.list.d/uaa-clevis23.sources"));
        assert!(cmds[2].contains("cat > /etc/apt/preferences.d/99-uaa-clevis23-pkcs11"));
    }
}
