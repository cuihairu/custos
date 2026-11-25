//! Pure argv builders and parsers for the power-policy adapters (todo.md
//! power probe family). Ungated like the ifconfig parser so the fixture
//! tests run on every platform; the live adapters (windows/macos) stay
//! cfg-gated and call these.

/// Builds one `powercfg /change standby-timeout-<ac|dc> <minutes>` argv.
/// `0` disables automatic standby on that power source.
pub fn powercfg_standby_argv(ac: bool, minutes: u64) -> Vec<String> {
    vec![
        "powercfg".into(),
        "/change".into(),
        if ac {
            "standby-timeout-ac".into()
        } else {
            "standby-timeout-dc".into()
        },
        minutes.to_string(),
    ]
}

/// Builds the `pmset -a disablesleep <0|1>` argv.
pub fn pmset_disablesleep_argv(disable: bool) -> Vec<String> {
    vec![
        "pmset".into(),
        "-a".into(),
        "disablesleep".into(),
        if disable { "1".into() } else { "0".into() },
    ]
}

/// Builds the `pmset schedule wake|poweron at MM/dd/yy HH:MM:SS` argv from
/// the local datetime string.
pub fn pmset_schedule_wake_argv(kind: &str, when: &str) -> Vec<String> {
    vec![
        "pmset".into(),
        "schedule".into(),
        kind.into(),
        "at".into(),
        when.into(),
    ]
}

/// Parses `date -r <epoch> +%m/%d/%y\ %H:%M:%S` output — the two-step form
/// that renders an epoch into pmset's expected local datetime. `None` on
/// anything that is not exactly `NN/NN/NN NN:NN:NN`.
pub fn parse_date_r(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let (d, t) = trimmed.split_once(' ')?;
    let dparts: Vec<&str> = d.split('/').collect();
    if dparts.len() != 3
        || dparts
            .iter()
            .any(|p| p.len() != 2 || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let tparts: Vec<&str> = t.split(':').collect();
    if tparts.len() != 3
        || tparts
            .iter()
            .any(|p| p.len() != 2 || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powercfg_argv_targets_both_power_sources() {
        assert_eq!(
            powercfg_standby_argv(true, 0),
            vec!["powercfg", "/change", "standby-timeout-ac", "0"]
        );
        assert_eq!(
            powercfg_standby_argv(false, 30),
            vec!["powercfg", "/change", "standby-timeout-dc", "30"]
        );
    }

    #[test]
    fn pmset_argvs_are_explicit() {
        assert_eq!(
            pmset_disablesleep_argv(true),
            vec!["pmset", "-a", "disablesleep", "1"]
        );
        assert_eq!(
            pmset_schedule_wake_argv("wake", "11/26/26 08:00:00"),
            vec!["pmset", "schedule", "wake", "at", "11/26/26 08:00:00"]
        );
    }

    #[test]
    fn date_r_parses_only_pmset_shape() {
        assert_eq!(
            parse_date_r("11/26/26 08:00:00\n"),
            Some("11/26/26 08:00:00".into())
        );
        assert_eq!(parse_date_r("Nov 26 2026\n"), None);
        assert_eq!(parse_date_r("26/11/2026 8:00:00\n"), None);
        assert_eq!(parse_date_r(""), None);
    }
}
