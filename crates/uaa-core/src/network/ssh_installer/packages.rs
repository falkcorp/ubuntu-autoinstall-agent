// file: crates/uaa-core/src/network/ssh_installer/packages.rs
// version: 3.0.0
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

/// Suite (Ubuntu 26.10 "Stonking Stingray") that ships clevis 23, the first
/// clevis with a pkcs11 pin. 26.04 "resolute" only has `20-1ubuntu2`
/// (universe), and `resolute-backports` does NOT carry clevis.
///
/// # RELEASE ONLY — never `stonking-proposed`
///
/// There are TWO builds of clevis 23 in 26.10 and the difference is the whole
/// ballgame:
///
/// * `23-1` in the **release** pocket — linked against OpenSSL 3, which 26.04
///   already has. `libssl4` is never pulled.
/// * `23-1build1` in **stonking-proposed** — rebuilt against OpenSSL 4,
///   `Depends: libssl4 (>= 4.0.0)`, which drags in the 26.10
///   `openssl-provider-legacy` over the 26.04 one.
///
/// Measured on a stock 26.04 container: pinning the release suite installs
/// `clevis`/`clevis-luks`/`clevis-tpm2` `23-1`, leaves `libssl4` absent,
/// `openssl-provider-legacy` at the 26.04 `3.5.5-1ubuntu3.2` and `libssl3t64`
/// intact, and `openssl list -providers` / `dgst` / a TLS handshake / `apt
/// update` all keep working. Adding `-proposed` to this string is the single
/// edit that turns that clean install into an OpenSSL 4 migration, so
/// [`clevis23_sources_content`] has a test asserting the word never appears.
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
/// `libssl4` and `openssl-provider-legacy` were once on this list and have been
/// REMOVED. They were only ever needed by `23-1build1` from
/// **stonking-proposed**, which is rebuilt against OpenSSL 4; the release-pocket
/// `23-1` this list actually resolves is linked against OpenSSL 3 and satisfies
/// itself from 26.04's own `libssl3t64`. Measured: after installing clevis 23-1
/// on stock 26.04, `libssl4` is absent and `openssl-provider-legacy` is
/// untouched at the 26.04 version. See [`CLEVIS23_SUITE`] for the
/// release-vs-proposed distinction, which is the non-obvious part.
pub const CLEVIS23_PINNED_PACKAGES: &[&str] = &[
    "clevis",
    "clevis-luks",
    "clevis-dracut",
    "clevis-systemd",
    "clevis-tpm2",
];

/// PKCS#11 / PIV token userspace. Deliberately NOT in
/// `CLEVIS23_PINNED_PACKAGES`: these install from plain 26.04 universe at their
/// stock versions.
///
/// `opensc`/`opensc-pkcs11` are in fact pulled automatically as a *Recommends*
/// of clevis 23 (measured: `0.27.0~rc1-1` arrives unasked), so naming `opensc`
/// here is belt-and-braces — it survives `--no-install-recommends` and makes the
/// dependency explicit rather than incidental. `pcscd` is NOT recommended by
/// anything and must be installed explicitly, or the pkcs11 pin has no daemon to
/// reach the token through.
pub const PKCS11_TOKEN_PACKAGES: &[&str] = &["opensc", "pcscd"];

/// Render the deb822 source for the clevis-23 pocket.
///
/// `Components: main universe` — clevis lives in *universe*; *main* is kept so
/// an ordinary library dependency of the clevis binaries can still resolve from
/// the same suite. Dropping *universe* makes the whole thing a silent no-op that
/// resolves clevis 20.
///
/// `Suites` is the RELEASE pocket and nothing else — see [`CLEVIS23_SUITE`].
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
         # clevis (= 23-1), and so on down the chain), so the allowlist must\n\
         # name the whole set - pinning one package alone is unresolvable.\n\
         #\n\
         # libssl4 / openssl-provider-legacy are deliberately NOT listed. They\n\
         # are needed only by 23-1build1 in {suite}-PROPOSED, which is rebuilt\n\
         # against OpenSSL 4. The release-pocket 23-1 pinned here links against\n\
         # OpenSSL 3 and is satisfied by the libssl3t64 in 26.04 - measured, see\n\
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
        // clevis is in universe; main stays for ordinary library deps.
        assert!(s.contains("Components: main universe"), "{s}");
        assert!(
            s.contains("Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg"),
            "deb822 without Signed-By fails apt update outright: {s}"
        );
        assert!(s.contains("Types: deb"), "{s}");
        // Must survive `bash -lc '…'` wrapping.
        assert!(!s.contains('\''), "no single quotes in generated content");
    }

    /// The ONE word that separates a clean install from an OpenSSL 4 migration.
    ///
    /// 26.10 carries two builds of clevis 23: `23-1` in the release pocket,
    /// linked against OpenSSL 3 (which 26.04 already has), and `23-1build1` in
    /// `-proposed`, rebuilt against OpenSSL 4 and therefore `Depends: libssl4
    /// (>= 4.0.0)`, which drags the 26.10 `openssl-provider-legacy` in over the
    /// 26.04 one. Measured on stock 26.04: pinning the release pocket installs
    /// clevis 23-1 with `libssl4` never pulled and `openssl-provider-legacy`
    /// untouched. Adding `-proposed` here — a plausible-looking "get the newest
    /// build" edit — silently reverses all of that, and nothing else in the
    /// suite would fail to reveal it.
    #[test]
    fn sources_never_enable_the_proposed_pocket() {
        let s = clevis23_sources_content(CLEVIS23_SUITE, CLEVIS23_MIRROR);
        assert!(
            !s.contains("proposed"),
            "the -proposed pocket carries the OpenSSL-4 rebuild of clevis 23; \
             the release pocket's 23-1 is the one that installs cleanly on \
             26.04: {s}"
        );
        // And the suite constant itself, since that is what a future edit would
        // most likely touch.
        assert!(
            !CLEVIS23_SUITE.contains("proposed"),
            "CLEVIS23_SUITE must name the release pocket only"
        );

        // Negative control: the assertion above is not passing because the
        // renderer drops whatever suite it is handed.
        let bad = clevis23_sources_content("stonking-proposed", CLEVIS23_MIRROR);
        assert!(
            bad.contains("proposed"),
            "the renderer must actually emit its suite, or this test is vacuous"
        );
    }

    /// The OpenSSL-4 packages are gone from the allowlist and must stay gone —
    /// pinning them would re-admit the very upgrade the release pocket avoids.
    #[test]
    fn preferences_do_not_pin_the_openssl4_packages() {
        let p = clevis23_preferences_content(CLEVIS23_SUITE);
        for pkg in ["libssl4", "openssl-provider-legacy"] {
            assert!(
                !CLEVIS23_PINNED_PACKAGES.contains(&pkg),
                "{pkg} must not be in the allowlist"
            );
            // The comment block explains WHY they are absent, so only the
            // `Package:` allowlist line itself is checked.
            let allow_line = p
                .lines()
                .find(|l| l.starts_with("Package: clevis"))
                .expect("allowlist line must exist");
            assert!(!allow_line.contains(pkg), "{pkg} in: {allow_line}");
        }
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
