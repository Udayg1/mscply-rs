use base64::{Engine as _, engine::general_purpose};
use libmpv2::Mpv;
use reqwest::header::USER_AGENT;
use serde_json;
use serde_json::Value;
use std::io::{Write, stdin, stdout};

const AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:145.0) Gecko/20100101 Firefox/145.0";
const QUERYBASE: &str = "https://maus.qqdl.site/search/?s=";
// const REFERER: &str = "https://tidal.squid.wtf/";
const STREAM: &str = "https://tidal.kinoplus.online/track/?";

async fn get_song(id: i32, audio_quality: &str) -> Result<Value, reqwest::Error> {
    let fin_url = format!("{}id={}&quality={}", STREAM, id, audio_quality);
    let client = reqwest::Client::new();
    let body: Value = client
        .get(fin_url)
        .header(USER_AGENT, AGENT)
        .send()
        .await?
        .json()
        .await?;
    Ok(body)
}

async fn search_result(query: &str) -> Result<Value, reqwest::Error> {
    let s: Vec<&str> = query.split(' ').collect();
    let q = format!("{}{}", QUERYBASE, s.join("%20"));
    let client = reqwest::Client::new();
    let body: Value = client
        .get(q)
        .header(USER_AGENT, AGENT)
        .send()
        .await?
        .json()
        .await?;
    Ok(body)
}

fn decode_base64(encoded: &str) -> String {
    let stripped = encoded.trim();
    let mut t = stripped.replace("-", "+").replace("_", "/");
    let missing = t.len() % 4;
    if missing == 1 {
        return String::from(stripped);
    } else if missing == 2 {
        t = format!("{}==", t);
    } else if missing == 3 {
        t = format!("{}=", t);
    }
    let decoded = general_purpose::STANDARD.decode(&t).unwrap();
    return String::from_utf8(decoded).unwrap();
}

fn queue_mpd_song(mpv: &mut Mpv, mpd: &str) {
    use std::fs::OpenOptions;
    use std::io::Write;

    // Write MPD to a file
    let path = "/tmp/mpv_queue.mpd";
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .unwrap();
    writeln!(f, "{}", mpd).unwrap();
    f.flush().unwrap(); // make sure MPV sees the complete file

    // Now queue the playlist
    queue_song(mpv, path);
}

fn queue_song(mpv: &mut Mpv, url: &str) {
    let idle: bool = mpv.get_property("idle_active").unwrap_or(true);
    if idle {
        let _ = mpv.command("loadfile", &[url, "replace"]);
    } else {
        let _ = mpv.command("loadfile", &[url, "append"]);
    }
}

#[tokio::main]
async fn main() {
    let mut mpv = match Mpv::new() {
        Ok(player) => player,
        Err(e) => {
            eprintln!("Failed to start MPV: {}", e);
            return;
        }
    };
    mpv.set_property("msg-level", "all=info").unwrap();
    mpv.set_property("log-file", "/tmp/mpv_playback.log").unwrap();
    mpv.set_property("demuxer-lavf-o", "protocol_whitelist=[file,https,http,tls,tcp,crypto,data]").unwrap();

    // println!("{}",hh);
    loop {
        print!("Enter song name (or q to quit): ");
        stdout().flush().unwrap();
        let mut name = String::new();
        stdin().read_line(&mut name).unwrap();
        let name = name.trim();
        if name.eq_ignore_ascii_case("q") {
            break;
        }
        let data = search_result(name).await.unwrap();
        println!("Results: ");
        let items = data
            .get("data")
            .and_then(|d| d.get("items"))
            .and_then(|arr| arr.as_array());
        if let Some(items) = items {
            let items = &items[..items.len().min(5)];
            for (i, track) in items.iter().enumerate() {
                let title = track
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown Title");
                let artist = track
                    .get("artist")
                    .and_then(|a| a.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown Artist");
                println!("{}. {} - {}", i + 1, title, artist);
            }

            print!("\nSelect track number: ");
            stdout().flush().unwrap();
            let mut input = String::new();
            stdin().read_line(&mut input).unwrap();
            let choice: usize = input.trim().parse().unwrap_or(0);
            if choice > 0 && choice <= items.len() {
                let track = &items[choice - 1];
                let id: i32 = track
                    .get("id")
                    .and_then(Value::as_i64)
                    .map(|v| v as i32)
                    .unwrap_or(0);
                let mut audio_quality: &str = track
                    .get("audioQuality")
                    .and_then(Value::as_str)
                    .unwrap_or("LOSSLESS");
                let tags = track
                    .get("mediaMetadata")
                    .and_then(|v| v.get("tags"))
                    .and_then(Value::as_array)
                    .unwrap();
                let qual = "HIRES_LOSSLESS";
                if tags.iter().any(|v| v.as_str() == Some(qual)) {
                    audio_quality = "HI_RES_LOSSLESS";
                }
                let song = get_song(id, audio_quality).await.unwrap();
                let manifest = song
                    .get("data")
                    .and_then(|v| v.get("manifest"))
                    .and_then(Value::as_str);
                let decoded = decode_base64(manifest.unwrap());
                if decoded.starts_with("<?xml") {
                    queue_mpd_song(&mut mpv, &decoded);
                } else {
                    if let Ok(json) = serde_json::from_str::<Value>(&decoded) {
                        if let Some(urls) = json.get("urls").and_then(|v| v.as_array()) {
                            if let Some(first_url) = urls.first().and_then(Value::as_str) {
                                queue_song(&mut mpv, first_url);
                            } else {
                                println!("'urls' array is empty or first element is not a string");
                            }
                        } else {
                            println!("No 'urls' array found");
                        }
                    }
                }
            }
        } else {
            continue;
        }
    }
}
