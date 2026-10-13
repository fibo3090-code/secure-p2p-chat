//! Whatever is typed at the TUI prompt, the parser answers with a command or an
//! error — never a panic, and never a command built from nonsense.

use p2pem_classic::tui::app::{TuiApp, TuiCommand};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn any_typed_line_parses_or_errors(line in ".{0,200}") {
        let _ = TuiApp::parse_command(&line);
        let _ = TuiApp::parse_command(&format!(":{line}"));
    }

    #[test]
    fn any_command_word_with_any_arguments_parses_or_errors(
        word in prop::sample::select(vec![
            "connect", "host", "relay", "join", "rename", "set", "send", "accept",
            "decline", "import", "party-connect", "party-trust", "party-post",
            "party-channel", "lock", "help", "contact", "block", "delete",
        ]),
        args in "[ -~\u{00A0}-\u{FFFF}]{0,80}",
    ) {
        let _ = TuiApp::parse_command(&format!(":{word} {args}"));
    }
}

#[test]
fn edge_inputs_are_answered_not_crashed_on() {
    for line in [
        "",
        ":",
        "::",
        ": ",
        ":connect",
        ":connect :",
        ":connect [",
        ":connect []:1",
        ":connect host:0",
        ":connect host:99999",
        ":host 99999",
        ":host -1",
        ":host abc",
        ":set",
        ":set notifications",
        ":set notifications maybe",
        ":rename",
        &format!(":rename {}", "😀".repeat(10_000)),
        &":".repeat(10_000),
        "\u{0}\u{1}\u{7f}",
    ] {
        let _ = TuiApp::parse_command(line);
    }
}

/// The TUI kept its own address parser, which left the brackets on an IPv6
/// host — `[::1]` — and no socket can dial a hostname with brackets in it.
#[test]
fn an_ipv6_connect_dials_the_bare_address() {
    assert_eq!(
        TuiApp::parse_command(":connect [::1]:9000").unwrap(),
        TuiCommand::Connect {
            host: "::1".to_string(),
            port: 9000,
        }
    );
}

#[test]
fn a_connect_to_an_undialable_address_is_refused() {
    for line in [
        ":connect host:0",
        ":connect []:80",
        ":connect http://example.com:80",
    ] {
        assert!(
            !matches!(TuiApp::parse_command(line), Ok(TuiCommand::Connect { .. })),
            "{line:?} produced a connect command"
        );
    }
}
