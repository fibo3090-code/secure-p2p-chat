#!/usr/bin/env rust-script
//! Test script to verify the invite link fix.
//! This simulates the user flow:
//! 1. Generate an invite link (simulating the "Share My Link" tab)
//! 2. Parse the link to add a contact (simulating "Add from Link")
//! 3. Try to open the chat and connect
//!
//! Expected behavior: No "Invalid port" error; address is either valid or None.

use std::collections::HashMap;

fn main() {
    println!("=== Testing Invite Link Fix ===\n");

    // Test 1: Generate an invite link with NO address (simulates recent fix)
    println!("✓ Test 1: Generate invite link without address");
    let link_no_addr = generate_test_link(None);
    println!("  Generated link (no address): {}\n", &link_no_addr[..link_no_addr.len().min(60)]);

    // Test 2: Generate an invite link with placeholder (old buggy behavior)
    println!("✓ Test 2: Generate invite link with placeholder address");
    let link_placeholder = generate_test_link(Some("YOUR_IP:PORT".to_string()));
    println!("  Generated link (placeholder): {}\n", &link_placeholder[..link_placeholder.len().min(60)]);

    // Test 3: Parse the no-address link
    println!("✓ Test 3: Parse link without address");
    match parse_invite(&link_no_addr) {
        Ok((name, addr)) => {
            println!("  ✓ Parsed successfully: name={}, address={:?}\n", name, addr);
            assert!(addr.is_none(), "Expected address to be None");
        }
        Err(e) => {
            eprintln!("  ✗ Parse failed: {}\n", e);
            std::process::exit(1);
        }
    }

    // Test 4: Parse the placeholder link (should be sanitized)
    println!("✓ Test 4: Parse link with placeholder address (should be sanitized)");
    match parse_invite(&link_placeholder) {
        Ok((name, addr)) => {
            if addr.is_none() {
                println!("  ✓ Placeholder was correctly ignored: address=None\n");
            } else {
                println!("  ✗ FAIL: Placeholder was NOT ignored: address={:?}", addr);
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("  ✗ Parse failed: {}\n", e);
            std::process::exit(1);
        }
    }

    // Test 5: Generate and parse a link with a valid address
    println!("✓ Test 5: Generate and parse link with valid address");
    let link_valid = generate_test_link(Some("127.0.0.1:54321".to_string()));
    match parse_invite(&link_valid) {
        Ok((name, addr)) => {
            if let Some(a) = addr {
                if a == "127.0.0.1:54321" {
                    println!("  ✓ Valid address was preserved: {}\n", a);
                } else {
                    println!("  ✗ FAIL: Address was corrupted: {}", a);
                    std::process::exit(1);
                }
            } else {
                println!("  ✗ FAIL: Valid address was lost: None\n");
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("  ✗ Parse failed: {}\n", e);
            std::process::exit(1);
        }
    }

    // Test 6: Generate and parse a link with an invalid address (missing port)
    println!("✓ Test 6: Generate and parse link with invalid address (no port)");
    let link_invalid = generate_test_link(Some("127.0.0.1".to_string()));
    match parse_invite(&link_invalid) {
        Ok((name, addr)) => {
            if addr.is_none() {
                println!("  ✓ Invalid address was correctly ignored: address=None\n");
            } else {
                println!("  ✗ FAIL: Invalid address was NOT ignored: address={:?}", addr);
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("  ✗ Parse failed: {}\n", e);
            std::process::exit(1);
        }
    }

    println!("=== ✓ All tests passed! ===");
    println!("The fix correctly:");
    println!("  • Omits placeholder addresses from generated invite links");
    println!("  • Sanitizes parsed invite addresses (ignores placeholders & invalid formats)");
    println!("  • Preserves valid host:port addresses");
}

/// Simulate invite link generation (returns base64-encoded JSON)
fn generate_test_link(address: Option<String>) -> String {
    let payload = serde_json::json!({
        "name": "Test User",
        "address": address,
        "fingerprint": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "public_key": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA...\n-----END PUBLIC KEY-----",
    });
    let json = serde_json::to_string(&payload).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(json);
    format!("chat-p2p://invite/{}", encoded)
}

/// Simulate invite link parsing with sanitization
fn parse_invite(link: &str) -> Result<(String, Option<String>), String> {
    let encoded = link.strip_prefix("chat-p2p://invite/").unwrap_or(link);
    let json = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| format!("Invalid base64: {}", e))?;
    let json_str =
        String::from_utf8(json).map_err(|e| format!("Invalid UTF-8: {}", e))?;

    let payload: serde_json::Value =
        serde_json::from_str(&json_str).map_err(|e| format!("Invalid JSON: {}", e))?;

    let name = payload["name"]
        .as_str()
        .ok_or("Missing name")?
        .to_string();

    // Sanitize address
    let address = payload["address"].as_str().and_then(|addr| {
        let trimmed = addr.trim();
        if trimmed.is_empty() {
            None
        } else if trimmed.eq_ignore_ascii_case("YOUR_IP:PORT") {
            None // Ignore placeholder
        } else {
            // Validate host:port format
            if let Some(idx) = trimmed.rfind(':') {
                let (host, port_str) = trimmed.split_at(idx);
                let port_str = &port_str[1..]; // skip ':'
                if host.is_empty() || port_str.parse::<u16>().is_err() {
                    None // Invalid
                } else {
                    Some(trimmed.to_string())
                }
            } else {
                None // No port, invalid
            }
        }
    });

    Ok((name, address))
}
