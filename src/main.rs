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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use noveler::{combine_txt, download_novel, Czbooks, Hjwzw, Novel543, Piaotia, Qbtr, UUkanshu};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 小說目錄網址
    #[arg(short, long, required = true)]
    url_contents: String,
    /// Cloudflare 認證 cookies，需先從瀏覽器取得
    #[arg(short, long)]
    cf_clearance: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let dir = env::current_exe().expect("find exe path");
    let dir = dir.parent().expect("have parent dir");

    let headers = args.cf_clearance.map(|cf_clearance| {
        header::HeaderMap::from_iter([(
            header::COOKIE,
            header::HeaderValue::from_str(&format!("cf_clearance={cf_clearance}"))
                .expect("create header value cf_clearance ok"),
        )])
    });

    let chapter_dir = get_novel(&args.url_contents, headers, dir).await;
    combine_txt(&chapter_dir).expect("combine txt ok");
}

async fn get_novel(url_contents: &str, headers: Option<header::HeaderMap>, dir: &Path) -> PathBuf {
    let result = match url_contents {
        _ if url_contents.starts_with("https://tw.hjwzw.com/") => {
            download_novel(
                Arc::new(Hjwzw::new(url_contents).expect("create Hjwzw ok")),
                url_contents,
                headers,
                dir,
                10,
                Duration::from_millis(0),
            )
            .await
        }
        _ if url_contents.starts_with("https://www.piaotia.com/") => {
            download_novel(
                Arc::new(Piaotia::new(url_contents).expect("create Piaotia ok")),
                url_contents,
                headers,
                dir,
                10,
                Duration::from_millis(0),
            )
            .await
        }
        _ if url_contents.starts_with("https://uukanshu.cc/") => {
            download_novel(
                Arc::new(UUkanshu::new(url_contents).expect("create UUkanshu ok")),
                url_contents,
                headers,
                dir,
                10,
                Duration::from_millis(0),
            )
            .await
        }
        _ if url_contents.starts_with("https://czbooks.net/") => {
            download_novel(
                Arc::new(Czbooks::new().expect("create Czbooks ok")),
                url_contents,
                headers,
                dir,
                1,
                Duration::from_millis(1000),
            )
            .await
        }
        _ if url_contents.starts_with("https://www.novel543.com/") => {
            download_novel(
                Arc::new(Novel543::new(url_contents).expect("create Novel543 ok")),
                url_contents,
                headers,
                dir,
                1,
                Duration::from_millis(1000),
            )
            .await
        }
        _ if url_contents.starts_with("https://www.qbtr.cc/") => {
            download_novel(
                Arc::new(Qbtr::new(url_contents).expect("create Qbtr ok")),
                url_contents,
                headers,
                dir,
                10,
                Duration::from_millis(0),
            )
            .await
        }
        url => panic!("Not support {url}"),
    };

    result.expect("download ok")
}
