//! What users actually paste into "address" fields, against the one parser
//! every connect path goes through.

use messenger_core::util::parse_host_port;

fn ok(input: &str, default: Option<u16>) -> (String, u16) {
    parse_host_port(input, default).unwrap_or_else(|e| panic!("{input:?} was refused: {e}"))
}

fn refused(input: &str, default: Option<u16>) {
    if let Ok(parsed) = parse_host_port(input, default) {
        panic!("{input:?} (default {default:?}) was accepted as {parsed:?}");
    }
}

#[test]
fn ordinary_addresses_parse() {
    assert_eq!(ok("example.com:80", None), ("example.com".into(), 80));
    assert_eq!(ok("  example.com:80 \n", None), ("example.com".into(), 80));
    assert_eq!(ok("127.0.0.1:1", None), ("127.0.0.1".into(), 1));
    assert_eq!(ok("[::1]:65535", None), ("::1".into(), 65535));
    assert_eq!(ok("[fe80::1%eth0]:80", None), ("fe80::1%eth0".into(), 80));
    assert_eq!(ok("::1", Some(12345)), ("::1".into(), 12345));
    assert_eq!(ok("2001:db8::7", Some(9)), ("2001:db8::7".into(), 9));
    assert_eq!(
        ok("example.com", Some(12345)),
        ("example.com".into(), 12345)
    );
    assert_eq!(ok("[::1]", Some(7)), ("::1".into(), 7));
}

#[test]
fn nothing_to_dial_is_refused() {
    for default in [None, Some(12345)] {
        refused("", default);
        refused("   ", default);
        refused(":80", default);
        refused("[]:80", default);
        refused("[]", default);
    }
}

#[test]
fn ports_that_cannot_be_dialled_are_refused() {
    refused("example.com:0", None);
    refused("example.com:65536", None);
    refused("example.com:-1", None);
    refused("example.com:+80", None);
    refused("example.com:", None);
    refused("[::1]:0", None);
}

#[test]
fn pasted_urls_and_garbage_are_not_hosts() {
    for default in [None, Some(12345)] {
        refused("http://example.com:8080", default);
        refused("chat-p2p://invite/abc", default);
        refused("example.com:80:90", default);
        refused("ex ample.com:80", default);
        refused("[::1]x", default);
        refused("[::1:80", default);
    }
    refused("::1", None);
}
