#![forbid(unsafe_code)]
#![warn(
    clippy::pedantic,
    missing_copy_implementations,
    missing_debug_implementations,
    //missing_docs,
    rustdoc::broken_intra_doc_links,
    trivial_numeric_casts,
    unused_allocation
)]
#![allow(
    clippy::missing_errors_doc,
    clippy::implicit_hasher,
    clippy::similar_names,
    clippy::module_name_repetitions
)]
mod noveler;

use clap::Parser;
use reqwest::header;
use std::env;
use std::path::Path;
use std::process;

use noveler::{combine_txt, download_novel, NovelError, PLATFORMS};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 小說目錄網址
    #[arg(short, long, required = true)]
    url_contents: String,
    /// Cloudflare `cf_clearance` cookie 值，需先從瀏覽器開發者工具取得
    #[arg(long)]
    cf_clearance: Option<String>,
    /// Cloudflare 場景下的瀏覽器 User-Agent，必須與取得 `cf_clearance` 時的瀏覽器完全一致
    #[arg(long)]
    cf_ua: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let dir = env::current_exe().expect("find exe path");
    let dir = dir.parent().expect("have parent dir");

    // 若平台需要 CF clearance 但使用者沒有提供，提前警告
    if let Some(config) = PLATFORMS
        .iter()
        .find(|p| args.url_contents.starts_with(p.prefix))
    {
        if config.requires_cf_clearance && args.cf_clearance.is_none() {
            eprintln!(
                "警告：此平台 ({}) 受 Cloudflare 保護，建議提供 --cf-clearance 與 --cf-ua",
                config.prefix
            );
            eprintln!("取得方式：瀏覽器開啟網站通過驗證後，從開發者工具 Application > Cookies 複製 cf_clearance 值");
            eprintln!("並從 Network > 任意請求的 User-Agent header 複製對應的 UA 字串");
        }
    }

    let headers = args.cf_clearance.map(|cf_clearance| {
        header::HeaderMap::from_iter([(
            header::COOKIE,
            header::HeaderValue::from_str(&format!("cf_clearance={cf_clearance}"))
                .expect("create header value cf_clearance ok"),
        )])
    });

    match get_novel(&args.url_contents, headers, args.cf_ua.as_ref(), dir).await {
        Ok(chapter_dir) => {
            if let Err(e) = combine_txt(&chapter_dir).await {
                eprintln!("Error combining txt: {e}");
                process::exit(1);
            }
        }
        Err(NovelError::CloudflareBlock(code)) => {
            eprintln!("錯誤：被 Cloudflare 封鎖 (HTTP {code})");
            eprintln!("請提供 --cf-clearance 與 --cf-ua 參數後重試");
            eprintln!("取得方式：瀏覽器開啟網站通過驗證後，從開發者工具 Application > Cookies 複製 cf_clearance 值");
            process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

async fn get_novel(
    url_contents: &str,
    headers: Option<header::HeaderMap>,
    cf_ua: Option<&String>,
    dir: &Path,
) -> Result<std::path::PathBuf, NovelError> {
    let config = PLATFORMS
        .iter()
        .find(|p| url_contents.starts_with(p.prefix))
        .ok_or_else(|| NovelError::UnsupportedUrl(url_contents.to_string()))?;

    let noveler = (config.factory)(url_contents)?;

    download_novel(
        noveler,
        url_contents,
        headers,
        cf_ua,
        dir,
        config.limit,
        config.interval,
    )
    .await
}
