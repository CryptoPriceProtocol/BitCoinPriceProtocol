use std::net::UdpSocket;
use std::time::UNIX_EPOCH;
use std::thread;

use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use serde_json::Value;

#[derive(Serialize, Deserialize)]
struct Price {
    usd: u64,          // micro-dollars
    volume24h: u64,    // satoshis
    sources: u32,      // bitmap
    ts: u64,           // unix seconds
    #[serde(with = "BigArray")]
    sig: [u8; 64],     // Ed25519 signature
}

fn fetch_from_exchange(url: &str, parse_fn: impl Fn(&Value) -> Option<f64>) -> Option<f64> {
    for attempt in 1..=3 {
        let client = reqwest::blocking::Client::builder()
            .user_agent("Mozilla/5.0 (compatible; BPP-POC/1.0)")
            .build()
            .unwrap();
        match client.get(url).send() {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(json) = resp.json::<Value>() {
                    if let Some(price) = parse_fn(&json) {
                        return Some(price);
                    }
                }
            }
            _ => {
                eprintln!("Attempt {} failed for {}: retrying...", attempt, url);
                thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }
    None
}

fn fetch_price() -> (u64, u64, u32) {
    // Kraken: result.XXBTZUSD.c[0] as string -> f64
    let kraken_parse = |json: &Value| {
        json["result"]["XXBTZUSD"]["c"][0].as_str().and_then(|s| s.parse::<f64>().ok())
    };
    let kraken_price = fetch_from_exchange("https://api.kraken.com/0/public/Ticker?pair=XBTUSD", kraken_parse);

    // Coinbase: data.amount as string -> f64
    let coinbase_parse = |json: &Value| {
        json["data"]["amount"].as_str().and_then(|s| s.parse::<f64>().ok())
    };
    let coinbase_price = fetch_from_exchange("https://api.coinbase.com/v2/prices/BTC-USD/spot", coinbase_parse);

    let avg_usd = match (kraken_price, coinbase_price) {
        (Some(k), Some(c)) => ((k + c) / 2.0 * 1_000_000.0) as u64,
        (Some(k), None) => (k * 1_000_000.0) as u64,
        (None, Some(c)) => (c * 1_000_000.0) as u64,
        (None, None) => 69420_370000, // Fallback $69420.37
    };

    let sources = match (kraken_price.is_some(), coinbase_price.is_some()) {
        (true, true) => 0b11,  // Both
        (true, false) => 0b01, // Kraken only
        (false, true) => 0b10, // Coinbase only
        (false, false) => 0b00, // Fallback
    };

    (avg_usd, 250_000_000_000_000, sources)
}

fn main() {
    if std::env::args().any(|a| a == "query") {
        client();
        return;
    }
    server();
}

fn server() {
    let socket = UdpSocket::bind("0.0.0.0:128").unwrap();
    let mut csprng = OsRng;
    let sk = SigningKey::generate(&mut csprng);
    let pk = sk.verifying_key();
    println!("Stratum-1 live on port 128 | pubkey {}", hex::encode(pk.as_bytes()));

    loop {
        let (usd, vol, src) = fetch_price();
        let ts = UNIX_EPOCH.elapsed().unwrap().as_secs();

        let mut p = Price { usd, volume24h: vol, sources: src, ts, sig: [0; 64] };
        let payload = bincode::serialize(&p).unwrap();
        p.sig = sk.sign(&payload).to_bytes();

        let packet = bincode::serialize(&p).unwrap();
        let mut buf = [0u8; 512];
        buf[..packet.len()].copy_from_slice(&packet);

        let mut recv = [0u8; 512];
        let (_, peer) = socket.recv_from(&mut recv).unwrap();
        socket.send_to(&buf[..packet.len()], peer).unwrap();

        println!("→ {} | ${:.2} ({} sources)", peer, usd as f64 / 1_000_000.0, src.count_ones());
    }
}

fn client() {
    let s = UdpSocket::bind("0.0.0.0:0").unwrap();
    s.send_to(&[0], "127.0.0.1:128").unwrap();

    let mut b = [0u8; 512];
    let (n, _) = s.recv_from(&mut b).unwrap();
    let p: Price = bincode::deserialize(&b[..n]).unwrap();

    println!(
        "BPP price: ${:.2} ±0 ({} sources)",
        p.usd as f64 / 1_000_000.0,
        p.sources.count_ones()
    );
}
