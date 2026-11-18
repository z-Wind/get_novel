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
use std::env;
use std::path::Path;
use std::process;

use noveler::{NovelError, PLATFORMS, combine_txt, download_novel};

#[derive(Parser, Debug)]
#[command(author, version, about = "基於 Chromiumoxide 的人機協同小說下載工具", long_about = None)]
struct Args {
    /// 小說目錄網址
    #[arg(short, long, required = true)]
    url_contents: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // 取得當前執行檔路徑作為下載暫存目錄
    let dir = env::current_exe().expect("無法選取執行檔路徑");
    let dir = dir.parent().expect("執行檔無上層目錄");

    println!("==================================================");
    println!("        📘 小說回憶備份工具 (Chromium 模式)        ");
    println!("==================================================");
    println!("💡 提示：本程式將會開啟一個自動化的 Chrome 瀏覽器視窗。");
    println!("💡 流程：瀏覽器會自動翻頁下載，若中途遇到驗證盾卡住，");
    println!("        請直接在瀏覽器上動手點選，並回到終端機按 Enter 即可！");
    println!("==================================================");

    // 啟動瀏覽器下載流程
    match get_novel(&args.url_contents, dir).await {
        Ok(chapter_dir) => {
            println!("📥 章節下載完畢，正在進行文本合併作業...");
            if let Err(e) = combine_txt(&chapter_dir).await {
                eprintln!("❌ 合併 TXT 檔案時失敗: {e}");
                process::exit(1);
            }
            println!("🎉 小說備份任務圓滿完成！");
        }
        Err(NovelError::UnsupportedUrl(url)) => {
            eprintln!("❌ 錯誤：目前不支援此網址類型：{url}");
            process::exit(1);
        }
        Err(e) => {
            eprintln!("❌ 程式執行期間發生異常錯誤: {e}");
            process::exit(1);
        }
    }
}

async fn get_novel(url_contents: &str, dir: &Path) -> Result<std::path::PathBuf, NovelError> {
    // 比對網址字首，尋找對應的支援平台
    let config = PLATFORMS
        .iter()
        .find(|p| url_contents.starts_with(p.prefix))
        .ok_or_else(|| NovelError::UnsupportedUrl(url_contents.to_string()))?;

    // 透過工廠函式建立對應平台的解析器
    let noveler = (config.factory)(url_contents)?;

    // 呼叫異步下載核心
    download_novel(noveler, url_contents, dir, config.delay_base_ms).await
}
