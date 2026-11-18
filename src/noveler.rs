mod czbooks;
mod hjwzw;
mod novel543;
mod piaotia;
mod qbtr;
mod uukanshu;

use chromiumoxide::browser::{Browser, BrowserConfig};
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use std::fmt::{self, Display};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use url::Url;
use visdom::types::Elements;

pub(crate) use czbooks::Czbooks;
pub(crate) use hjwzw::Hjwzw;
pub(crate) use novel543::Novel543;
pub(crate) use piaotia::Piaotia;
pub(crate) use qbtr::Qbtr;
pub(crate) use uukanshu::UUkanshu;

#[derive(Error, Debug)]
pub(crate) enum NovelError {
    #[error("{0} can not be found")]
    NotFound(String),
    #[error("parse fail {0}")]
    ParseError(#[from] url::ParseError),
    #[error("{0} can not be a base")]
    CannotBeABase(String),
    #[error("std io fail {0}")]
    StdIOError(#[from] io::Error),
    #[error("visdom fail {0}")]
    VisdomError(#[from] visdom::types::BoxDynError),
    #[error("Regex fail {0}")]
    RegexError(#[from] regex::Error),
    #[error("Browser error: {0}")]
    BrowserError(String),
    #[error("URL {0} not supported")]
    UnsupportedUrl(String),
}

#[derive(Debug, PartialEq)]
pub(crate) struct Book {
    pub(crate) name: String,
    pub(crate) author: String,
}

impl fmt::Display for Book {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}_{}", self.author, self.name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Chapter {
    pub(crate) order: String,
    pub(crate) title: String,
    pub(crate) text: String,
}

impl Chapter {
    pub(crate) fn content(&self) -> String {
        format!("{}\n\n{}", self.title, self.text)
    }
}

/// 平台下載設定。新增平台只需在 PLATFORMS 加一筆。
pub(crate) struct PlatformConfig {
    pub prefix: &'static str,
    pub delay_base_ms: u64, // 基礎延遲毫秒數
    pub factory: fn(&str) -> Result<Arc<dyn Noveler>, NovelError>,
}

/// 所有已支援平台的設定表
pub(crate) static PLATFORMS: &[PlatformConfig] = &[
    PlatformConfig {
        prefix: "https://hjwzw.com",
        delay_base_ms: 1000,
        factory: |url| Ok(Arc::new(Hjwzw::new(url)?)),
    },
    PlatformConfig {
        prefix: "https://piaotia.com",
        delay_base_ms: 2000,
        factory: |url| Ok(Arc::new(Piaotia::new(url)?)),
    },
    PlatformConfig {
        prefix: "https://uukanshu.cc",
        delay_base_ms: 2000,
        factory: |url| Ok(Arc::new(UUkanshu::new(url)?)),
    },
    PlatformConfig {
        prefix: "https://czbooks.net",
        delay_base_ms: 2000,
        factory: |_url| Ok(Arc::new(Czbooks::new()?)),
    },
    PlatformConfig {
        prefix: "https://novel543.com",
        delay_base_ms: 2000,
        factory: |url| Ok(Arc::new(Novel543::new(url)?)),
    },
    PlatformConfig {
        prefix: "https://qbtr.cc",
        delay_base_ms: 1000,
        factory: |url| Ok(Arc::new(Qbtr::new(url)?)),
    },
];

pub trait Noveler: Display + Sync + Send + 'static {
    fn get_book_info(&self, document: &Elements) -> Result<Book, NovelError>;
    fn get_chapter_urls_sorted(&self, document: &Elements) -> Result<Vec<Url>, NovelError>;

    fn append_urls_with_orders(&self, urls: Vec<Url>) -> Vec<(String, Url)> {
        urls.into_iter()
            .enumerate()
            .map(|(i, url)| (format!("{:05}", i + 1), url))
            .collect()
    }

    fn get_chapter(&self, document: &Elements, order: &str) -> Result<Chapter, NovelError>;
    fn get_next_page(&self, document: &Elements) -> Result<Option<Url>, NovelError>;
    fn process_chapter(&self, chapter: Chapter) -> Chapter;
}

fn file_name(order: &str) -> String {
    format!("{order}.txt")
}

/// 透過單次目錄讀取建立已存在檔案集合，避免在章節多時產生大量磁碟 stat 系統呼叫
async fn remove_url_with_exist_file(
    urls: Vec<(String, Url)>,
    dir: &Path,
) -> Result<Vec<(String, Url)>, NovelError> {
    let mut existing_files = std::collections::HashSet::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if let Ok(name) = entry.file_name().into_string() {
            existing_files.insert(name);
        }
    }

    Ok(urls
        .into_iter()
        .filter(|(order, _)| !existing_files.contains(&file_name(order)))
        .collect())
}

async fn save_chapter(chapter: Chapter, dir: &Path, pb: &ProgressBar) -> Result<(), NovelError> {
    let path = dir.join(file_name(&chapter.order));
    tokio::fs::write(path, chapter.content()).await?;
    pb.inc(1);
    Ok(())
}

/// 暫停並提示使用者在實體 Chrome 視窗完成 Cloudflare 驗證
/// 若傳入 ProgressBar，會先暫停進度條渲染以避免終端機畫面錯亂，並安全隔離同步輸入
async fn wait_for_user_cf_verification(
    context_msg: &str,
    pb: Option<&ProgressBar>,
) -> Result<(), NovelError> {
    // 轉為擁有所有權的 String，滿足 spawn_blocking 所需的 'static 生命週期限制
    let msg = context_msg.to_string();

    let prompt_and_wait = move || {
        eprintln!("\n🛑 [警告] {msg}，可能遇到了 Cloudflare 驗證盾！");
        eprintln!("💡 請切換到跳出的 Chrome 視窗，手動完成點擊驗證。");
        eprintln!("👉 驗證通過並看到目標畫面後，回到這裡按下 [Enter] 鍵繼續...");

        // 通知 Tokio 排程器此處會短暫阻塞，確保背景 Chrome WebSocket handler 不受影響
        tokio::task::block_in_place(|| {
            let mut input = String::new();
            let _ = io::stdin().read_line(&mut input);
        });

        println!("🔄 收到指令，正在重新讀取網頁...");
    };

    if let Some(bar) = pb {
        bar.suspend(prompt_and_wait);
    } else {
        tokio::task::spawn_blocking(prompt_and_wait)
            .await
            .map_err(|e| NovelError::BrowserError(format!("讀取使用者輸入失敗: {e}")))?;
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn download_novel(
    noveler: Arc<dyn Noveler>,
    url_contents: &str,
    dir: &Path,
    delay_base_ms: u64,
) -> Result<PathBuf, NovelError> {
    // 偵測當前是否處於 cargo test 測試環境下
    // 如果是測試環境，強迫開啟 headless 隱形模式，避免因為沒有 GUI 桌面導致 Chrome 無法開啟分頁
    let is_test = std::env::var("CARGO_MANIFEST_DIR").is_ok() && cfg!(test);

    let mut config_builder =
        BrowserConfig::builder().arg("--disable-blink-features=AutomationControlled");

    // chromiumoxide 預設就是無頭隱形模式
    // 只有在非測試環境（真正的 CLI 下載任務）下，才需要主動呼叫 .with_head() 彈出實體視窗
    if !is_test {
        config_builder = config_builder.with_head();
    }

    let config = config_builder
        .window_size(1280, 800)
        .build()
        .map_err(NovelError::BrowserError)?;

    if !is_test {
        println!("🚀 正在啟動本機 Chrome 瀏覽器...");
    }

    let (mut browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|e| NovelError::BrowserError(e.to_string()))?;

    // 啟動背景 Task 維持與 Chrome 的 WebSocket 連線
    tokio::spawn(async move { while handler.next().await.is_some() {} });

    // 新啟動的瀏覽器有時候分頁資料庫還在加載，使用簡單的非同步重試確保百分之百拿到初始分頁
    let mut page = None;
    for _ in 0..5 {
        if let Ok(pages) = browser.pages().await
            && let Some(p) = pages.first().cloned()
        {
            page = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    // 如果還是撈不到預設分頁，就手動叫瀏覽器全新建立一個分頁（保底機制）
    let page = match page {
        Some(p) => p,
        None => browser
            .new_page("about:blank")
            .await
            .map_err(|e| NovelError::BrowserError(e.to_string()))?,
    };

    if !is_test {
        println!("🌐 導航至目錄頁：{url_contents}");
    }

    page.goto(url_contents)
        .await
        .map_err(|e| NovelError::BrowserError(e.to_string()))?;

    tokio::time::sleep(Duration::from_millis(1000)).await;

    let (document, book) = loop {
        let html = page
            .content()
            .await
            .map_err(|e| NovelError::BrowserError(e.to_string()))?;
        let doc = visdom::Vis::load(html)?;

        if let Ok(b) = noveler.get_book_info(&doc)
            && !b.author.is_empty()
            && !b.name.is_empty()
        {
            break (doc, b);
        }

        if is_test {
            return Err(NovelError::BrowserError(
                "測試環境下無法讀取小說基本資訊。".to_string(),
            ));
        }

        wait_for_user_cf_verification("無法讀取小說目錄資訊", None).await?;
    };

    let dir = dir
        .join("temp")
        .join(noveler.to_string())
        .join(book.to_string());
    tokio::fs::create_dir_all(dir.as_path()).await?;

    let urls = noveler.get_chapter_urls_sorted(&document)?;
    let urls = noveler.append_urls_with_orders(urls);

    let total_book_chapters = urls.len() as u64;
    let urls = remove_url_with_exist_file(urls, &dir).await?;
    let remaining_chapters = urls.len() as u64;
    let already_downloaded = total_book_chapters.saturating_sub(remaining_chapters);

    if !is_test {
        println!(
            "🎬 任務：{book} (總計: {total_book_chapters} 章 | 已完成: {already_downloaded} 章 | 剩餘: {remaining_chapters} 章)",
        );
    }

    let pb = if is_test {
        ProgressBar::hidden()
    } else {
        let bar = ProgressBar::new(total_book_chapters);
        bar.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({percent}%) | 剩餘預估: {eta} | {msg}",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("#>-"),
        );
        bar.set_position(already_downloaded);
        bar
    };

    for (order, mut current_url) in urls {
        let mut retry_success = false;

        // 用來累積儲存「同一個章節」內所有分頁的文字與標題
        let mut combined_title = String::new();
        let mut combined_text = String::new();

        while !retry_success {
            pb.set_message(format!("下載中 [{order}]"));

            // 加入 15 秒跳轉逾時保護，避免網路卡死導致整個程式無限掛起
            let goto_res =
                tokio::time::timeout(Duration::from_secs(15), page.goto(current_url.as_str()))
                    .await;

            if !matches!(goto_res, Ok(Ok(_))) {
                pb.println(format!("⚠️ [{order}] 網頁載入超時或失敗，2 秒後重試..."));
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }

            // 結合奈秒時間與章節序號雜湊產生更自然的隨機延遲 (500ms ~ 1500ms)
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(500, |d| u64::from(d.subsec_nanos()));
            let jitter = (nanos % 1000) + 500;
            tokio::time::sleep(Duration::from_millis(delay_base_ms + jitter)).await;

            let ch_html = page
                .content()
                .await
                .map_err(|e| NovelError::BrowserError(e.to_string()))?;
            let ch_doc = visdom::Vis::load(ch_html)?;

            // 嘗試解析章節內容
            if let Ok(chapter) = noveler.get_chapter(&ch_doc, &order) {
                // 如果是這個章節的第一頁，記錄標題
                if combined_title.is_empty() {
                    combined_title = chapter.title;
                }

                // 累積累積內文 (中間用換行隔開)
                if !combined_text.is_empty() {
                    combined_text.push_str("\n\n");
                }
                combined_text.push_str(&chapter.text);

                // 核心邏輯：檢查是否有章節內部的「下一頁」網址
                match noveler.get_next_page(&ch_doc) {
                    Ok(Some(next_page_url)) => {
                        pb.println(format!("🔗 [{order}] 偵測到章節分頁，繼續載入下一頁..."));
                        current_url = next_page_url;
                    }
                    Ok(None) => {
                        let final_chapter = Chapter {
                            order: order.clone(),
                            title: combined_title.clone(),
                            text: combined_text.clone(),
                        };

                        // 丟入各平台自訂的清洗邏輯並儲存
                        let final_chapter = noveler.process_chapter(final_chapter);
                        save_chapter(final_chapter, &dir, &pb).await?;
                        retry_success = true;
                    }
                    Err(e) => {
                        pb.println(format!("⚠️ [{order}] 解析下一頁網址錯誤: {e}"));
                        retry_success = true;
                    }
                }
            } else {
                if is_test {
                    return Err(NovelError::BrowserError(format!(
                        "測試環境下無法解析章節 [{order}] 內文。"
                    )));
                }
                wait_for_user_cf_verification("無法解析網頁內文", Some(&pb)).await?;
            }
        }
    }

    pb.finish_with_message("所有章節下載完成！");
    let _ = browser.close().await;

    Ok(dir)
}

/// 將目錄頁資料夾內的所有獨立 .txt 依序高效合併（使用緩衝區寫入並保持輸出精簡）
pub(crate) async fn combine_txt(dir: &Path) -> Result<(), NovelError> {
    let mut save_path = dir.to_path_buf();
    save_path.set_extension("txt");

    let mut entries = tokio::fs::read_dir(dir).await?;
    let mut paths: Vec<PathBuf> = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "txt") && path.is_file() {
            paths.push(path);
        }
    }
    paths.sort_unstable();

    let total_files = paths.len();
    let save_path_clone = save_path.clone();

    tokio::task::spawn_blocking(move || -> Result<(), NovelError> {
        let file = fs::File::create(&save_path_clone)?;
        let mut writer = io::BufWriter::with_capacity(256 * 1024, file);

        for path in &paths {
            let mut input = fs::File::open(path)?;
            io::copy(&mut input, &mut writer)?;
            writer.write_all(b"\n\n")?;
        }
        writer.flush()?;
        Ok(())
    })
    .await
    .map_err(|e| NovelError::StdIOError(io::Error::other(e.to_string())))??;

    println!(
        "📝 已將 {} 個章節合併至：{}",
        total_files,
        save_path.display()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
    use regex::Regex;
    use reqwest::IntoUrl;
    use std::{
        env,
        sync::atomic::{AtomicI32, Ordering as AtomicOrdering},
    };
    use tempdir::TempDir;

    use super::*;

    async fn guess_coding<T: IntoUrl>(url: T) -> (&'static encoding_rs::Encoding, bool) {
        let resp = reqwest::get(url).await.unwrap();
        let body_bytes = resp.bytes().await.unwrap();
        let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
        detector.feed(&body_bytes, true);
        (detector.guess(None, Utf8Detection::Allow), true)
    }

    #[ignore = "Used for detecting coding"]
    #[tokio::test]
    async fn test_guess_coding() {
        dbg!(guess_coding("https://www.qbtr.cc/tongren/3655.html").await);
    }

    struct FakeNoveler {
        re: Regex,
        host: String,
        num: AtomicI32,
    }

    impl FakeNoveler {
        fn new(host: String) -> Self {
            Self {
                re: Regex::new(r"text").expect("pattern"),
                host,
                num: AtomicI32::new(1),
            }
        }
    }

    impl Display for FakeNoveler {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "FakeNoveler")
        }
    }

    impl Noveler for FakeNoveler {
        fn get_book_info(&self, _document: &Elements) -> Result<Book, NovelError> {
            Ok(Book {
                name: "name".to_string(),
                author: "author".to_string(),
            })
        }

        fn get_chapter_urls_sorted(&self, _document: &Elements) -> Result<Vec<Url>, NovelError> {
            Ok((1..)
                .take(10)
                .map(|n| Url::parse(&format!("{}/{n}", self.host)).unwrap())
                .collect())
        }

        fn get_chapter(&self, _document: &Elements, order: &str) -> Result<Chapter, NovelError> {
            Ok(Chapter {
                order: order.to_string(),
                title: format!("title_{order}"),
                text: format!("text_{order}"),
            })
        }

        fn get_next_page(&self, _document: &Elements) -> Result<Option<Url>, NovelError> {
            let num = self.num.fetch_add(1, AtomicOrdering::Relaxed);
            if num > 10 {
                Ok(None)
            } else {
                Ok(Some(Url::parse(&format!("{}/next_page/{num}", self.host))?))
            }
        }

        fn process_chapter(&self, chapter: Chapter) -> Chapter {
            Chapter {
                text: self
                    .re
                    .replace_all(&chapter.text, "text_process")
                    .to_string(),
                ..chapter
            }
        }
    }

    #[tokio::test]
    async fn test_save_chapter() {
        let dir = TempDir::new("noveler_test_save_chapter").unwrap();

        let chapter = Chapter {
            order: "order".to_string(),
            title: "title".to_string(),
            text: "text".to_string(),
        };
        save_chapter(chapter.clone(), dir.path(), &ProgressBar::hidden())
            .await
            .unwrap();

        let file_path = dir.path().join(file_name(&chapter.order));
        assert!(file_path.is_file());
        assert_eq!(
            tokio::fs::read_to_string(&file_path).await.unwrap(),
            "title\n\ntext"
        );
    }

    #[tokio::test]
    async fn test_basic_noveler() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();
        let dir = TempDir::new("noveler_test_basic_noveler").unwrap();

        // 幫 Mock 伺服器註冊根目錄網址（/），並給予一個合法的 HTML 內文
        // 這樣 Chrome 導航進去時才不會彈出 ERR_HTTP_RESPONSE_CODE_FAILURE 錯誤
        let _contents_mock = server
            .mock("GET", "/")
            .with_status(200)
            .with_header("content-type", "text/html; charset=utf-8")
            .with_body(
                "<html><head><title>測試小說目錄</title></head><body><h1>目錄頁</h1></body></html>",
            )
            .create_async()
            .await;

        // 因為 FakeNoveler 內部會去抓 1..10 的章節網址，
        // 我們需要幫這 10 個章節的網址（如 /1, /2...）也註冊好合法的網頁回應，讓 Chrome 線性下載不卡住
        let mut chapter_mocks = Vec::new();
        for n in 1..=10 {
            let path_string = format!("/{n}");
            let next_page_string = format!("/next_page/{n}");

            // 1. 主章節頁
            let mock1 = server
                .mock("GET", path_string.as_str())
                .with_status(200)
                .with_header("content-type", "text/html; charset=utf-8")
                .with_body(format!("<html><body><div class=\"name\">標題_{n}</div><div class=\"content\">內文_{n}</div></body></html>"))
                .create_async()
                .await;

            // 2. 補上對應的章節內下一頁 Mock，讓 Chrome 有東西可以載入
            let mock2 = server
                .mock("GET", next_page_string.as_str())
                .with_status(200)
                .with_header("content-type", "text/html; charset=utf-8")
                .with_body(format!("<html><body><div class=\"name\">標題_{n}_n</div><div class=\"content\">內文_{n}_n</div></body></html>"))
                .create_async()
                .await;

            chapter_mocks.push(mock1);
            chapter_mocks.push(mock2);
        }

        let chapter_dir = download_novel(
            Arc::new(FakeNoveler::new(url.clone())) as Arc<dyn Noveler>,
            url.as_str(),
            dir.path(),
            1000,
        )
        .await
        .unwrap();

        for n in 1..=10u32 {
            assert!(
                dir.path()
                    .join(format!("temp/FakeNoveler/author_name/{n:05}.txt"))
                    .exists()
            );
        }
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("temp/FakeNoveler/author_name/00001.txt"))
                .await
                .unwrap(),
            "title_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001"
        );

        combine_txt(&chapter_dir).await.unwrap();
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("temp/FakeNoveler/author_name.txt"))
                .await
                .unwrap(),
            "title_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntext_process_00001\n\ntitle_00002\n\ntext_process_00002\n\ntitle_00003\n\ntext_process_00003\n\ntitle_00004\n\ntext_process_00004\n\ntitle_00005\n\ntext_process_00005\n\ntitle_00006\n\ntext_process_00006\n\ntitle_00007\n\ntext_process_00007\n\ntitle_00008\n\ntext_process_00008\n\ntitle_00009\n\ntext_process_00009\n\ntitle_00010\n\ntext_process_00010\n\n"
        );

        dir.close().unwrap();
    }

    #[ignore = "Online Test sometimes with env cf_clearance for Cloudflare"]
    #[tokio::test]
    async fn test_czbooks() {
        let dir = TempDir::new("noveler_test_czbooks").unwrap();
        let url = "https://czbooks.net/n/uhemc";
        let noveler = Czbooks::new().expect("create Czbooks ok");
        let chapter_dir =
            download_novel(Arc::new(noveler) as Arc<dyn Noveler>, url, dir.path(), 1000)
                .await
                .expect("download ok");
        assert!(chapter_dir.ends_with("射手兇猛"), "dir = {chapter_dir:?}");
        combine_txt(&chapter_dir).await.expect("combine txt ok");
        dir.close().unwrap();
    }

    #[ignore = "Online Test"]
    #[tokio::test]
    async fn test_hjwzw() {
        let dir = TempDir::new("noveler_test_hjwzw").unwrap();
        let url = "https://tw.hjwzw.com/Book/Chapter/48386";
        let noveler = Hjwzw::new(url).expect("create Hjwzw ok");
        let chapter_dir =
            download_novel(Arc::new(noveler) as Arc<dyn Noveler>, url, dir.path(), 1000)
                .await
                .expect("download ok");
        assert!(chapter_dir.ends_with("射手兇猛"), "dir = {chapter_dir:?}");
        combine_txt(&chapter_dir).await.expect("combine txt ok");
        dir.close().unwrap();
    }

    #[ignore = "Online Test"]
    #[tokio::test]
    async fn test_novel543() {
        let dir = TempDir::new("noveler_test_novel543").unwrap();
        let url = "https://www.novel543.com/0413188175/dir";
        let noveler = Novel543::new(url).expect("create Novel543 ok");
        let chapter_dir =
            download_novel(Arc::new(noveler) as Arc<dyn Noveler>, url, dir.path(), 1000)
                .await
                .expect("download ok");
        assert!(chapter_dir.ends_with("射手兇猛"), "dir = {chapter_dir:?}");
        combine_txt(&chapter_dir).await.expect("combine txt ok");
        dir.close().unwrap();
    }

    #[ignore = "Online Test"]
    #[tokio::test]
    async fn test_piaotia() {
        let dir = TempDir::new("noveler_test_piaotia").unwrap();
        let url = "https://www.piaotia.com/html/14/14881/";
        let noveler = Piaotia::new(url).expect("create Piaotia ok");
        let chapter_dir =
            download_novel(Arc::new(noveler) as Arc<dyn Noveler>, url, dir.path(), 1000)
                .await
                .expect("download ok");
        assert!(chapter_dir.ends_with("射手兇猛"), "dir = {chapter_dir:?}");
        combine_txt(&chapter_dir).await.expect("combine txt ok");
        dir.close().unwrap();
    }

    #[ignore = "Online Test"]
    #[tokio::test]
    async fn test_qbtr() {
        let dir = TempDir::new("noveler_test_qbtr").unwrap();
        let url = "https://www.qbtr.cc/tongren/3655.html";
        let noveler = Qbtr::new(url).expect("create Qbtr ok");
        let chapter_dir =
            download_novel(Arc::new(noveler) as Arc<dyn Noveler>, url, dir.path(), 1000)
                .await
                .expect("download ok");
        assert!(chapter_dir.ends_with("射手兇猛"), "dir = {chapter_dir:?}");
        combine_txt(&chapter_dir).await.expect("combine txt ok");
        dir.close().unwrap();
    }

    #[ignore = "Online Test"]
    #[tokio::test]
    async fn test_uukanshu() {
        let dir = TempDir::new("noveler_test_uukanshu").unwrap();
        let url = "https://uukanshu.cc/book/20692/";
        let noveler = UUkanshu::new(url).expect("create UUkanshu ok");
        let chapter_dir =
            download_novel(Arc::new(noveler) as Arc<dyn Noveler>, url, dir.path(), 1000)
                .await
                .expect("download ok");
        assert!(chapter_dir.ends_with("射手兇猛"), "dir = {chapter_dir:?}");
        combine_txt(&chapter_dir).await.expect("combine txt ok");
        dir.close().unwrap();
    }

    #[ignore = "Used for HTML parser benchmark comparison"]
    #[test]
    fn test_compare_parser() {
        let html = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/czbooks/contents.html"
        ));
        let selector = r"ul.nav.chapter-list > li > a";
        let n = 100;

        {
            let start = std::time::Instant::now();
            for _ in 0..n {
                let document = nipper::Document::from(html);
                let a = document
                    .select(selector)
                    .iter()
                    .next()
                    .unwrap()
                    .attr("href")
                    .unwrap()
                    .to_string();
                assert_eq!(a, "//czbooks.net/n/uilla7/und20");
            }
            println!("nipper {:?}", start.elapsed());
        }
        {
            let start = std::time::Instant::now();
            for _ in 0..n {
                let document = scraper::Html::parse_document(html);
                let sel = scraper::Selector::parse(selector).unwrap();
                let a = document
                    .select(&sel)
                    .next()
                    .unwrap()
                    .value()
                    .attr("href")
                    .unwrap();
                assert_eq!(a, "//czbooks.net/n/uilla7/und20");
            }
            println!("scraper {:?}", start.elapsed());
        }
        {
            let start = std::time::Instant::now();
            for _ in 0..n {
                let document = visdom::Vis::load(html).unwrap();
                let a = document
                    .find(selector)
                    .first()
                    .attr("href")
                    .unwrap()
                    .to_string();
                assert_eq!(a, "//czbooks.net/n/uilla7/und20");
            }
            println!("visdom {:?}", start.elapsed());
        }
        {
            let start = std::time::Instant::now();
            for _ in 0..n {
                let document = accessibility_scraper::Html::parse_document(html);
                let sel = accessibility_scraper::Selector::parse(selector).unwrap();
                let a = document
                    .select(&sel)
                    .next()
                    .unwrap()
                    .value()
                    .attr("href")
                    .unwrap();
                assert_eq!(a, "//czbooks.net/n/uilla7/und20");
            }
            println!("accessibility-scraper {:?}", start.elapsed());
        }
        {
            let start = std::time::Instant::now();
            for _ in 0..n {
                let document = crabquery::Document::from(html);
                let a = document
                    .select(selector)
                    .first()
                    .unwrap()
                    .attr("href")
                    .unwrap();
                assert_eq!(a, "//czbooks.net/n/uilla7/und20");
            }
            println!("crabquery {:?}", start.elapsed());
        }
    }
}
