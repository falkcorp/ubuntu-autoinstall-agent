// file: src/autoinstall/render.rs
// version: 1.0.0
// guid: c2d3e4f5-a6b7-8c9d-0e1f-2a3b4c5d6e7f
// last-edited: 2026-06-21

//! Render a subiquity autoinstall `user-data` from a template + [`HostSpec`].
//!
//! The renderer is a deliberately dumb, data-driven text substitution. The
//! template is the source of truth for everything that does NOT vary per host;
//! the [`HostSpec`] supplies the bits that do. The default template
//! (`templates/len-serv.user-data.tmpl`) is the hand-verified len-serv-003
//! config with these placeholders:
//!
//! | Placeholder              | HostSpec field          | Example                                   |
//! |--------------------------|-------------------------|-------------------------------------------|
//! | `{{HOSTNAME}}`           | `hostname`              | `len-serv-003`                            |
//! | `{{NET_ADDRESS}}`        | `network_address`       | `172.16.3.96/23`                          |
//! | `{{COCKROACH_ADVERTISE}}`| `cockroach_advertise`   | `172.16.3.96:36357`                       |
//! | `{{COCKROACH_JOIN}}`     | `cockroach_join`        | `172.16.2.30:36357,172.16.3.92:36357,...` |
//!
//! Rendering substitutes every known placeholder, then checks that no
//! `{{...}}` token remains — an unfilled placeholder in a custom template is a
//! hard error rather than something that silently ships to a machine.

use crate::autoinstall::host_spec::HostSpec;
use crate::error::AutoInstallError;
use crate::Result;

/// The embedded default template: the hand-verified len-serv-003 user-data.
const DEFAULT_TEMPLATE: &str = include_str!("templates/len-serv.user-data.tmpl");

/// Return the embedded default template (the len-serv-003 config).
pub fn default_template() -> &'static str {
    DEFAULT_TEMPLATE
}

/// Render `template` against `spec`, substituting all known placeholders.
///
/// Errors if any `{{...}}` placeholder remains after substitution (i.e. the
/// template referenced something this renderer does not know how to fill).
pub fn render_user_data(template: &str, spec: &HostSpec) -> Result<String> {
    let rendered = template
        .replace("{{HOSTNAME}}", &spec.hostname)
        .replace("{{NET_ADDRESS}}", &spec.network_address)
        .replace("{{COCKROACH_ADVERTISE}}", &spec.cockroach_advertise)
        .replace("{{COCKROACH_JOIN}}", &spec.cockroach_join);

    if let Some(leftover) = find_placeholder(&rendered) {
        return Err(AutoInstallError::ConfigError(format!(
            "template contains unknown placeholder '{leftover}' that the renderer cannot fill"
        )));
    }

    Ok(rendered)
}

/// Find the first `{{...}}` placeholder in `s`, if any (returns the full token
/// including braces).
fn find_placeholder(s: &str) -> Option<String> {
    let start = s.find("{{")?;
    let rest = &s[start + 2..];
    let end = rest.find("}}")?;
    Some(format!("{{{{{}}}}}", &rest[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The golden fixtures: rendering the default template with each host's
    /// params must reproduce the exact bytes pulled from the live deployment.
    const GOLDEN_001: &str = include_str!("../../tests/fixtures/golden/len-serv-001.user-data");
    const GOLDEN_002: &str = include_str!("../../tests/fixtures/golden/len-serv-002.user-data");
    const GOLDEN_003: &str = include_str!("../../tests/fixtures/golden/len-serv-003.user-data");

    #[test]
    fn renders_003_byte_for_byte() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");
        let out = render_user_data(default_template(), &spec).unwrap();
        assert_eq!(out, GOLDEN_003);
    }

    #[test]
    fn renders_001_byte_for_byte() {
        let spec = HostSpec::for_lenserv("len-serv-001", "172.16.3.92/23");
        let out = render_user_data(default_template(), &spec).unwrap();
        assert_eq!(out, GOLDEN_001);
    }

    #[test]
    fn renders_002_byte_for_byte() {
        let spec = HostSpec::for_lenserv("len-serv-002", "172.16.3.94/23");
        let out = render_user_data(default_template(), &spec).unwrap();
        assert_eq!(out, GOLDEN_002);
    }

    #[test]
    fn unfilled_placeholder_is_an_error() {
        let custom = "hostname: {{HOSTNAME}}\nmystery: {{NOT_A_REAL_FIELD}}\n";
        let spec = HostSpec::for_lenserv("x", "10.0.0.1/24");
        let err = render_user_data(custom, &spec).unwrap_err();
        assert!(
            err.to_string().contains("{{NOT_A_REAL_FIELD}}"),
            "error should name the offending placeholder, got: {err}"
        );
    }

    #[test]
    fn default_template_has_no_residual_after_full_render() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");
        let out = render_user_data(default_template(), &spec).unwrap();
        assert!(find_placeholder(&out).is_none());
    }
}
