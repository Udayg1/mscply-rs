use base64::{Engine as _, engine::general_purpose};
use macros::*;
use regex::Regex;
use reqwest::Client;
use reqwest::header::{CONTENT_TYPE, REFERER, USER_AGENT};
use serde_json::{Value, json};
use std::io::Write;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use std::{env, fs};
use url;
use uuid;

pub static PREF_QUAL: OnceLock<String> = OnceLock::new();
static INFOSTREAM: AtomicBool = AtomicBool::new(true);
static CHECKED: AtomicBool = AtomicBool::new(false);
static CLIENT: OnceLock<Client> = OnceLock::new();
pub static IS_CACHING: AtomicBool = AtomicBool::new(false);
pub const AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:149.0) Gecko/20100101 Firefox/149.0";
pub static FALLBACK_STREAM: &str = "https://dzr.tabs-vs-spaces.wtf";
pub static FALLBACK: &str = "https://tidal-proxy.monochrome.tf/openapi/v2";
static SUGGESTION_SOURCE: &str =
    "https://monochrome1.cyrusna29.workers.dev/u/fd3963db40cc3d5fc0720b460be2";
static TOKEN: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));

pub struct CacheItem {
    pub path: String,
    pub index: usize,
}

struct JsData {
    client_id: String,
    client_secret: String,
}

fn token() -> String {
    TOKEN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_token(token: String) {
    let mut s = TOKEN.lock().unwrap();
    *s = token;
}

async fn get_js_name() -> Result<String, reqwest::Error> {
    let url = "https://lossless.wtf";
    let client = CLIENT.get().unwrap_or(&Client::new()).clone();
    let res = client.get(url).send().await?.text().await?;
    let re = Regex::new(r#"src=["'][^"']*/([^/"']+\.js)["']"#).unwrap();

    Ok(re
        .captures(&res)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or(String::new()))
}

async fn get_js_file() -> Result<String, reqwest::Error> {
    let js = get_js_name().await?;
    let url = format!("https://lossless.wtf/assets/{}", js);
    let client = CLIENT.get().unwrap_or(&Client::new()).clone();
    let res = client
        .get(url)
        .timeout(Duration::from_secs(6))
        .send()
        .await?
        .text()
        .await?;
    Ok(res)
}

fn extract_data_from_js<T: AsRef<str>>(js_string: T) -> Option<JsData> {
    let client_id_regex = Regex::new(r#"BROWSER_CLIENT_ID\s*=\s*['"]([^'"]+)['"]"#).unwrap();
    let client_secret_regex =
        Regex::new(r#"BROWSER_CLIENT_SECRET\s*=\s*['"]([^'"]+)['"]"#).unwrap();
    let client_id = client_id_regex
        .captures(js_string.as_ref())
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str());
    let client_secret = client_secret_regex
        .captures(js_string.as_ref())
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str());
    if client_id.is_none() || client_secret.is_none() {
        return None;
    }
    return Some(JsData {
        client_id: client_id.unwrap().to_string(),
        client_secret: client_secret.unwrap().to_string(),
    });
}

async fn get_token(js_data: &JsData) -> Result<String, reqwest::Error> {
    let url = "https://auth.tidal.com/v1/oauth2/token";
    let client = CLIENT.get().unwrap_or(&Client::new()).clone();
    let base64_encoded_data =
        make_base64(format!("{}:{}", js_data.client_id, js_data.client_secret));
    let request = client
        .post(url)
        .header("Authorization", format!("Basic {}", base64_encoded_data))
        .form(&[
            ("client_id", js_data.client_id.clone()),
            ("client_secret", js_data.client_secret.clone()),
            ("grant_type", "client_credentials".to_string()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    Ok(request
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string())
}

fn make_base64<T: AsRef<str>>(data: T) -> String {
    general_purpose::STANDARD.encode(data.as_ref())
}

pub fn infostream() -> bool {
    let d = INFOSTREAM.load(std::sync::atomic::Ordering::Relaxed);
    return d.clone();
}

pub fn checked() -> bool {
    let d = CHECKED.load(std::sync::atomic::Ordering::Relaxed);
    return d.clone();
}

pub async fn fallback_metadata(qobuz_id: &str) -> Value {
    let cli = CLIENT.get().unwrap().clone();
    let url = concat_strings(Vec::from([
        FALLBACK,
        "/api/get-music?offset=0&q=",
        qobuz_id,
    ]));
    let mut jsn = empty_json();
    let resp = cli
        .get(url)
        .header(USER_AGENT, AGENT)
        .header(REFERER, FALLBACK)
        .timeout(Duration::from_secs(7))
        .send()
        .await;
    let res = match resp {
        Ok(e) => e.error_for_status(),
        Err(_) => {
            return jsn;
        }
    };
    if res.is_err() {
        return jsn;
    }
    let resp = res.unwrap().text().await.unwrap();
    if let Ok(e) = serde_json::from_str::<Value>(&resp) {
        if let Some(m) = e
            .get("data")
            .and_then(|v| v.get("tracks"))
            .and_then(|v| v.get("items"))
            .and_then(Value::as_array)
            .and_then(|v| v.get(0))
        {
            if let Some(bit) = m.get("maximum_bit_depth").and_then(Value::as_i64) {
                if bit >= 16 {
                    jsn.as_object_mut()
                        .unwrap()
                        .insert("quality".to_string(), json!("flac"));
                } else {
                    jsn.as_object_mut()
                        .unwrap()
                        .insert("quality".to_string(), json!("opus"));
                }
            }
            if let Some(dur) = m.get("duration").and_then(Value::as_i64) {
                jsn.as_object_mut()
                    .unwrap()
                    .insert("duration".to_string(), json!(dur));
            }
        }
    }
    jsn
}

// pub async fn metadata(id: &str) -> Value {
//     let cli = CLIENT.get().unwrap().clone();
//     let url = concat_strings(Vec::from([API, "/metadata?asin=", id]));
//     let mut resp = None;
//     let res = cli
//         .get(url)
//         .header(USER_AGENT, AGENT)
//         .timeout(Duration::from_secs(7))
//         .send()
//         .await;
//     let response = match res {
//         Ok(e) => e.error_for_status(),
//         Err(_) => {
//             return empty_json();
//         }
//     };
//     if !response.is_err() {
//         resp = Some(response.unwrap().json::<Value>().await.expect("JSON ERROR"));
//     }
//     if resp.is_none() {
//         return empty_json();
//     }
//     let res = resp.expect("Status error");
//     let mut qual = None;
//     if let Some(qual_arr) = res
//         .get("trackList")
//         .and_then(Value::as_array)
//         .and_then(|v| v.first())
//         .and_then(|v| v.get("assetQualities"))
//         .and_then(Value::as_array)
//     {
//         for i in qual_arr {
//             if i.get("quality").and_then(Value::as_str) == Some("CD") {
//                 qual = Some("flac");
//                 break;
//             } else {
//                 qual = Some("opus");
//             }
//         }
//     }
//     let mut duration = None;
//     if let Some(dur) = res
//         .get("trackList")
//         .and_then(Value::as_array)
//         .and_then(|v| v.first())
//         .and_then(|v| v.get("duration"))
//         .and_then(Value::as_i64)
//     {
//         duration = Some(dur);
//     }
//     let pref = PREF_QUAL.get().unwrap();
//     if !qual.is_none() {
//         if qual == Some("flac") && pref == "HIGH" {
//             return json!({"quality":"opus", "duration": duration.unwrap_or(0)});
//         }
//         json!({"quality":qual.unwrap_or("opus"), "duration": duration.unwrap_or(0)})
//     } else {
//         json!({"quality": "opus"})
//     }
// }

pub fn cache_next_song(url: String, index: usize, sx: Sender<CacheItem>) {
    tokio::spawn(async move {
        let path = concat_strings(Vec::from([
            env::temp_dir().to_str().unwrap(),
            "/",
            &uuid::Uuid::new_v4().to_string(),
        ]));
        let path2 = path.to_string();
        let return_item = CacheItem {
            path: path2,
            index: index,
        };
        IS_CACHING.store(true, std::sync::atomic::Ordering::SeqCst);
        let cli = Client::new();

        if url.starts_with("<?xml") {
            let new: Vec<&str> = url.split(" ").collect();
            let mut init: String = "--".to_string();
            let mut r = "--";
            for i in new {
                if i.starts_with("media=") {
                    init = i[7..i.len() - 1].to_string();
                } else if i.starts_with("r=") {
                    let smth = i.split("/").collect::<Vec<&str>>()[0];
                    r = &smth[3..smth.len() - 1];
                }
            }
            init = init.replace("amp;", "");
            let new_init = init.split("$Number$").collect::<Vec<&str>>();
            let r = r.parse::<u32>().unwrap();
            let mut bytes: Vec<u8> = Vec::new();
            for i in 0..=r + 2 {
                let resp = cli
                    .get(concat_strings(Vec::from([
                        new_init[0],
                        &i.to_string(),
                        new_init[1],
                    ])))
                    .send()
                    .await;
                if resp.is_err() {
                    IS_CACHING.store(false, std::sync::atomic::Ordering::SeqCst);
                    sx.send(return_item).unwrap();
                    return;
                }
                let chunk = resp.unwrap().bytes().await.unwrap();
                bytes.extend_from_slice(&chunk);
            }
            let mut handle = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .open(&path)
                .unwrap();
            handle.write_all(&bytes).unwrap();
            IS_CACHING.store(false, std::sync::atomic::Ordering::SeqCst);
            sx.send(return_item).unwrap();
            return;
        } else {
            let bytes = cli.get(url).send().await.unwrap().bytes().await;
            fs::write(&path, bytes.unwrap()).ok();
            IS_CACHING.store(false, std::sync::atomic::Ordering::SeqCst);
            sx.send(return_item).unwrap();
            return;
        }
    });
}

pub fn set_url() {
    tokio::spawn(async {
        let cli = reqwest::Client::new();
        CLIENT.set(cli.clone()).unwrap();
        let js = get_js_file().await.unwrap_or(String::new());
        let mut js_data_op = extract_data_from_js(&js);
        // if js_data_op.is_none() {
        //     eprintln!("{js}");
        //     panic!("fked");
        // }
        while js_data_op.is_none() {
            let js = get_js_file().await.unwrap_or(String::new());
            js_data_op = extract_data_from_js(js);
        }
        let js_data = js_data_op.unwrap();
        let token = get_token(&js_data).await.unwrap();
        set_token(token);
        let mut last_token = Instant::now();
        loop {
            if last_token.elapsed() >= Duration::from_hours(1) {
                let token = get_token(&js_data).await.unwrap();
                set_token(token);
                last_token = Instant::now();
            }
            if let Ok(res) = cli
                .get(format!("{SUGGESTION_SOURCE}/stream/221144944"))
                .timeout(Duration::from_secs(10))
                .send()
                .await
            {
                if let Ok(_) = res.error_for_status() {
                    INFOSTREAM.store(true, std::sync::atomic::Ordering::Relaxed);
                } else {
                    INFOSTREAM.store(false, std::sync::atomic::Ordering::Relaxed);
                }
            } else {
                INFOSTREAM.store(false, std::sync::atomic::Ordering::Relaxed);
            }
            CHECKED.store(true, std::sync::atomic::Ordering::Release);
            tokio::time::sleep(Duration::from_mins(1)).await;
        }
    });
}

pub async fn get_song(id: &str, _: &str) -> Result<Value, reqwest::Error> {
    let fin_url = concat_strings(Vec::from([SUGGESTION_SOURCE, "/stream/", id]));
    let client = CLIENT.get().unwrap().clone();
    let mut body = None;
    if let Ok(b) = client
        .get(&fin_url)
        .header(USER_AGENT, AGENT)
        .header(REFERER, SUGGESTION_SOURCE)
        .send()
        .await
    {
        if let Ok(jsn) = b.error_for_status()?.json::<Value>().await {
            body = Some(jsn.clone())
        }
    }

    if body.is_none() {
        Ok(empty_json())
    } else {
        Ok(body.unwrap())
    }
}

pub async fn fallback_get_song(id: &str, audio_quality: &str) -> Result<Value, reqwest::Error> {
    Ok(
        json!({"data": {"url": format!("{}/stream/?isrc={}&format={}", FALLBACK_STREAM, id, if audio_quality == "flac" {"FLAC"} else {"MP3_320"})}}),
    )
}

pub async fn search(query: &str) -> Result<Value, reqwest::Error> {
    let mut q = url::Url::parse(SUGGESTION_SOURCE).unwrap();
    q.path_segments_mut().unwrap().push("search");
    q.query_pairs_mut().append_pair("q", query);
    let client = CLIENT.get().unwrap().clone();
    let mut body = None;
    if let Ok(b) = client
        .get(q)
        .header(USER_AGENT, AGENT)
        .header(REFERER, SUGGESTION_SOURCE)
        .send()
        .await
    {
        if let Ok(e) = b.error_for_status()?.json::<Value>().await {
            body = Some(e)
        }
    }

    if body.is_none() {
        return Ok(empty_json());
    }
    let body = body.unwrap();
    Ok(body)
}

pub async fn get_artist_from_tidal_id<T: AsRef<str>>(id: T) -> Result<Value, reqwest::Error> {
    let url = format!("{FALLBACK}/artists/{}", id.as_ref());
    let client = CLIENT.get().unwrap_or(&Client::new()).clone();
    let res = client
        .get(url)
        .header("Authorization", format!("Bearer {}", token()))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    Ok(res)
}

pub async fn fallback_search(query: &str) -> Result<Value, reqwest::Error> {
    let mut q = url::Url::parse(FALLBACK).unwrap();
    q.path_segments_mut().unwrap().push("searchResults");
    q.path_segments_mut().unwrap().push(query);
    q.query_pairs_mut().append_pair("limit", "10");

    let q = q.to_string().replace("+", "%20") + "&include=tracks%2Ctracks.artists";
    let client = CLIENT.get().unwrap().clone();
    let mut body = None;
    if let Ok(b) = client
        .get(q)
        .header(USER_AGENT, AGENT)
        .header("Authorization", format!("Bearer {}", token()))
        .send()
        .await
    {
        if let Ok(e) = b.error_for_status()?.text().await {
            if let Ok(data) = serde_json::from_str::<Value>(&e) {
                body = Some(data.clone());
            }
        }
    }
    if body.is_none() {
        return Ok(empty_json());
    }
    let body = body.unwrap();
    Ok(body)
}

pub async fn convert_to_ytm(name: &str) -> Option<String> {
    let client = CLIENT.get().unwrap().clone();

    let body = query_json(name);
    let new: Vec<&str> = name.trim().split(' ').collect();
    let mut res = client
        .post("https://music.youtube.com/youtubei/v1/search?prettyPrint=false")
        .header(USER_AGENT, AGENT)
        .header(CONTENT_TYPE, "application/json")
        .header(
            REFERER,
            concat_strings(Vec::from([
                "https://music.youtube.com/search?q=",
                new.join("+").as_str(),
            ])),
        )
        .timeout(Duration::from_secs(7))
        .json(&body)
        .send()
        .await;
    while res.is_err() {
        res = client
            .post("https://music.youtube.com/youtubei/v1/search?prettyPrint=false")
            .header(USER_AGENT, AGENT)
            .header(CONTENT_TYPE, "application/json")
            .header(
                REFERER,
                concat_strings(Vec::from([
                    "https://music.youtube.com/search?q=",
                    new.join("+").as_str(),
                ])),
            )
            .json(&body)
            .send()
            .await;
    }
    let ress = res.unwrap().json::<Value>().await;
    if ress.is_err() {
        return Some("".to_string());
    }
    let resn = ress.unwrap();
    let first: Option<String>;
    let arr = resn
        .get("contents")
        .and_then(|c| c.get("tabbedSearchResultsRenderer"))
        .and_then(|t| t.get("tabs"))
        .and_then(Value::as_array)
        .and_then(|tabs| tabs.get(0))
        .and_then(|tab| tab.get("tabRenderer"))
        .and_then(|tab| tab.get("content"))
        .and_then(|content| content.get("sectionListRenderer"))
        .and_then(|slr| slr.get("contents"))
        .and_then(Value::as_array)
        .unwrap();
    let zero_index = arr.get(0).unwrap();
    let first_index = arr.get(1);
    let con = zero_index.get("musicShelfRenderer");
    if !con.is_none() {
        first = con
            .unwrap()
            .get("contents")
            .and_then(Value::as_array)
            .and_then(|items| items.get(0))
            .and_then(|item| item.get("musicResponsiveListItemRenderer"))
            .and_then(|mr| mr.get("flexColumns"))
            .and_then(Value::as_array)
            .and_then(|cols| cols.get(0))
            .and_then(|col| col.get("musicResponsiveListItemFlexColumnRenderer"))
            .and_then(|flex| flex.get("text"))
            .and_then(|text| text.get("runs"))
            .and_then(Value::as_array)
            .and_then(|runs| runs.get(0))
            .and_then(|run| run.get("navigationEndpoint"))
            .and_then(|ne| ne.get("watchEndpoint"))
            .and_then(|we| we.get("videoId"))
            .and_then(Value::as_str)
            .map(|s| s.to_string());
    } else {
        first = first_index
            .unwrap()
            .get("musicShelfRenderer")
            .and_then(|msr| msr.get("contents"))
            .and_then(Value::as_array)
            .and_then(|items| items.get(0))
            .and_then(|item| item.get("musicResponsiveListItemRenderer"))
            .and_then(|mr| mr.get("flexColumns"))
            .and_then(Value::as_array)
            .and_then(|cols| cols.get(0))
            .and_then(|col| col.get("musicResponsiveListItemFlexColumnRenderer"))
            .and_then(|flex| flex.get("text"))
            .and_then(|text| text.get("runs"))
            .and_then(Value::as_array)
            .and_then(|runs| runs.get(0))
            .and_then(|run| run.get("navigationEndpoint"))
            .and_then(|ne| ne.get("watchEndpoint"))
            .and_then(|we| we.get("videoId"))
            .and_then(Value::as_str)
            .map(|s| s.to_string());
    }
    if !first.is_none() {
        first
    } else {
        Some("".to_string())
    }
}

pub async fn get_ytrecs(ytid: &str) -> Value {
    if ytid.is_empty() {
        return empty_json();
    }
    let client = CLIENT.get().unwrap().clone();
    let body = ytrecs_json(ytid);
    let resp = client
        .post("https://music.youtube.com/youtubei/v1/next?prettyPrint=false")
        .header("Content-Type", "application/json")
        .header(USER_AGENT, AGENT)
        .header(
            "Referer",
            concat_strings(Vec::from([
                "https://music.youtube.com/watch?v=",
                ytid,
                "&list=RDAMVM",
                ytid,
            ])),
        )
        .timeout(Duration::from_secs(10))
        .json(&body)
        .send()
        .await;
    let mut res = None;
    while resp.is_err() || res.is_none() {
        let resp = client
            .post("https://music.youtube.com/youtubei/v1/next?prettyPrint=false")
            .header("Content-Type", "application/json")
            .header(USER_AGENT, AGENT)
            .header(
                "Referer",
                concat_strings(Vec::from([
                    "https://music.youtube.com/watch?v=",
                    ytid,
                    "&list=RDAMVM",
                    ytid,
                ])),
            )
            .json(&body)
            .send()
            .await;
        if !resp.is_err() {
            res = Some(resp.unwrap().text().await.unwrap());
        }
    }
    serde_json::from_str(&res.unwrap()).unwrap()
}

pub async fn cache_url(id: &str, url: &str) -> Option<String> {
    let path = concat_strings(Vec::from([
        &env::var("HOME").unwrap(),
        "/.local/share/mscply/songs/",
        id,
    ]));
    if fs::metadata(&path).is_ok() {
        return Some(path);
    }

    let bytes = CLIENT
        .get()
        .unwrap()
        .clone()
        .get(url)
        .send()
        .await
        .ok()?
        .bytes()
        .await
        .ok()?;

    fs::write(&path, bytes).ok()?;
    Some(path)
}

pub fn get_ytrec_array(recs: Value) -> Option<Vec<Value>> {
    let tab0 = recs
        .get("contents")
        .and_then(|v| v.get("singleColumnMusicWatchNextResultsRenderer"))
        .and_then(|v| v.get("tabbedRenderer"))
        .and_then(|v| v.get("watchNextTabbedResultsRenderer"))
        .and_then(|v| v.get("tabs"))
        .and_then(Value::as_array)
        .and_then(|a| a.get(0))
        .cloned()
        .unwrap_or(empty_json());
    let cont = tab0
        .get("tabRenderer")
        .and_then(|v| v.get("content"))
        .and_then(|v| v.get("musicQueueRenderer"))
        .and_then(|v| v.get("content"))
        .and_then(|v| v.get("playlistPanelRenderer"))
        .and_then(|v| v.get("contents"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or(Vec::new());
    if cont.is_empty() {
        return None;
    }
    let mut arr = Vec::new();
    for i in cont.iter().skip(1) {
        let id = i
            .get("playlistPanelVideoRenderer")
            .and_then(|v| v.get("videoId"))
            .and_then(Value::as_str)
            .map(|s| s.to_string());
        let name = i
            .get("playlistPanelVideoRenderer")
            .and_then(|v| v.get("title"))
            .and_then(|v| v.get("runs"))
            .and_then(Value::as_array)
            .and_then(|v| v.get(0))
            .and_then(|v| v.get("text"));
        let artist = i
            .get("playlistPanelVideoRenderer")
            .and_then(|v| v.get("shortBylineText"))
            .and_then(|v| v.get("runs"))
            .and_then(Value::as_array)
            .and_then(|v| v.get(0))
            .and_then(|v| v.get("text"));
        let duration = i
            .get("playlistPanelVideoRenderer")
            .and_then(|v| v.get("lengthText"))
            .and_then(|v| v.get("runs"))
            .and_then(Value::as_array)
            .and_then(|v| v.get(0))
            .and_then(|v| v.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let time = duration
            .split(':')
            .rev()
            .enumerate()
            .map(|(j, value)| value.parse::<u64>().unwrap_or(0) * 60u64.pow(j as u32))
            .sum::<u64>();
        let jso = json!({"id": id, "name": name, "artist": artist, "duration": time});
        arr.push(jso);
    }
    Some(arr)
}

pub async fn get_suggestions(query: &str) -> Result<Value, reqwest::Error> {
    let s = query
        .split(' ')
        .collect::<Vec<&str>>()
        .join("%20")
        .to_string();
    let q = concat_strings(Vec::from([SUGGESTION_SOURCE, "search?q=", s.as_str()]));
    let client = CLIENT.get().unwrap().clone();
    let mut body = None;
    if let Ok(b) = client
        .get(q)
        .header(USER_AGENT, AGENT)
        .header(REFERER, SUGGESTION_SOURCE)
        .send()
        .await
    {
        if let Ok(e) = b.json::<Value>().await {
            body = Some(e)
        }
    }

    if body.is_none() {
        return Ok(empty_json());
    }
    let body = body.unwrap();
    Ok(body)
}
