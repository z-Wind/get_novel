mod czbooks;
mod hjwzw;
mod novel543;
mod piaotia;
mod qbtr;
mod uukanshu;

use futures::future::BoxFuture;
use reqwest::header;
use reqwest::{Client, IntoUrl};
use std::collections::HashMap;
use std::fmt::Display;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, io};
use thiserror::Error;
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;
use url::Url;
use visdom::types::Elements;

pub(crate) use czbooks::Czbooks;
pub(crate) use hjwzw::Hjwzw;
pub(crate) use novel543::Novel543;
pub(crate) use piaotia::Piaotia;
pub(crate) use qbtr::Qbtr;
pub(crate) use uukanshu::UUkanshu;

const USER_AGENT: &str = if cfg!(target_os = "macos") {
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36"
} else if cfg!(target_os = "linux") {
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36"
} else {
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36"
};

const MAX_RETRIES: u32 = 3;
const RETRY_BASE_DELAY_MS: u64 = 500;

#[derive(Error, Debug)]
pub(crate) enum NovelError {
    #[error("Some URLs is incomplete")]
    IncompleteUrl,
    #[error("{0} can not be found")]
    NotFound(String),
    #[error("parse fail {0}")]
    ParseError(#[from] url::ParseError),
    #[error("{0} can not be a base")]
    CannotBeABase(String),
    #[error("reqwest fail {0}")]
    ReqwestError(#[from] reqwest::Error),
    #[error("std io fail {0}")]
    StdIOError(#[from] io::Error),
    #[error("visdom fail {0}")]
    VisdomError(#[from] visdom::types::BoxDynError),
    #[error("Regex fail {0}")]
    RegexError(#[from] regex::Error),
    #[error("No {0}, may be blocked by Cloudflare")]
    BlockedByCloudflare(String),
    #[error(
        "Blocked by Cloudflare (HTTP {0}): pass --cf-clearance and --cf-ua matching your browser"
    )]
    CloudflareBlock(u16),
    #[error("URL {0} not supported")]
    UnsupportedUrl(String),
}

impl NovelError {
    /// 判斷是否值得重試
    fn is_retryable(&self) -> bool {
        match self {
            Self::ReqwestError(e) => {
                e.is_timeout() || e.is_connect() || e.status().is_some_and(|s| s.is_server_error())
            }
            _ => false,
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct Book {
    name: String,
    author: String,
}

impl fmt::Display for Book {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}_{}", self.author, self.name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Chapter {
    order: String,
    title: String,
    text: String,
}

impl Chapter {
    pub(crate) fn content(&self) -> String {
        format!("{}\n\n{}", self.title, self.text)
    }
}

/// 平台下載設定。新增平台只需在 PLATFORMS 加一筆。
pub(crate) struct PlatformConfig {
    pub prefix: &'static str,
    pub limit: usize,
    pub interval: Duration,
    /// 此平台是否需要 Cloudflare clearance cookie
    pub requires_cf_clearance: bool,
    pub factory: fn(&str) -> Result<Arc<dyn Noveler>, NovelError>,
}

/// 所有已支援平台的設定表
pub(crate) static PLATFORMS: &[PlatformConfig] = &[
    PlatformConfig {
        prefix: "https://tw.hjwzw.com/",
        limit: 10,
        interval: Duration::from_millis(0),
        requires_cf_clearance: false,
        factory: |url| Ok(Arc::new(Hjwzw::new(url)?)),
    },
    PlatformConfig {
        prefix: "https://www.piaotia.com/",
        limit: 1,
        interval: Duration::from_millis(1000),
        requires_cf_clearance: false,
        factory: |url| Ok(Arc::new(Piaotia::new(url)?)),
    },
    PlatformConfig {
        prefix: "https://uukanshu.cc/",
        limit: 1,
        interval: Duration::from_millis(0),
        requires_cf_clearance: true,
        factory: |url| Ok(Arc::new(UUkanshu::new(url)?)),
    },
    PlatformConfig {
        prefix: "https://czbooks.net/",
        limit: 1,
        interval: Duration::from_millis(1000),
        requires_cf_clearance: true,
        factory: |_url| Ok(Arc::new(Czbooks::new()?)),
    },
    PlatformConfig {
        prefix: "https://www.novel543.com/",
        limit: 1,
        interval: Duration::from_millis(1000),
        requires_cf_clearance: false,
        factory: |url| Ok(Arc::new(Novel543::new(url)?)),
    },
    PlatformConfig {
        prefix: "https://www.qbtr.cc/",
        limit: 10,
        interval: Duration::from_millis(0),
        requires_cf_clearance: false,
        factory: |url| Ok(Arc::new(Qbtr::new(url)?)),
    },
];

pub trait Noveler: Display + Sync + Send + 'static {
    fn need_encoding(&self) -> Option<&'static encoding_rs::Encoding> {
        None
    }

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

    fn fetch_chapter<'a>(
        &'a self,
        client: Client,
        order: &'a str,
        url: Url,
    ) -> BoxFuture<'a, Result<(Chapter, Option<Url>), NovelError>> {
        Box::pin(async move {
            let document = get_html_and_fix_encoding(client, url, self.need_encoding()).await?;
            let document = visdom::Vis::load(document)?;

            let chapter = self.get_chapter(&document, order)?;
            let chapter = self.process_chapter(chapter);

            let next_page = self.get_next_page(&document)?;

            Ok((chapter, next_page))
        })
    }
}

/// 代表一個等待下載的任務，附帶重試次數
#[derive(Debug)]
struct Task {
    order: String,
    url: Url,
    retry_count: u32,

    counter: Arc<AtomicUsize>,
    tx: mpsc::Sender<Self>,
}

impl Task {
    fn new(order: String, url: Url, counter: Arc<AtomicUsize>, tx: mpsc::Sender<Self>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self {
            order,
            url,
            retry_count: 0,
            counter,
            tx,
        }
    }

    fn with_retry(mut self) -> Self {
        self.retry_count += 1;

        self
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        let prev = self.counter.fetch_sub(1, Ordering::Release);

        if prev > 0 {
            println!("{:<10} => {:05}", "Pending", prev);
        }
    }
}

enum TaskStatus {
    Processing,
    Success,
}

fn file_name(order: &str) -> String {
    format!("{order}.txt")
}

/// 將目錄頁的所有章節 URL 送入 channel，回傳任務總數
fn enqueue_tasks(
    noveler: &Arc<dyn Noveler>,
    document: &Elements,
    dir: &Path,
    tx: mpsc::Sender<Task>,
    pending: Arc<AtomicUsize>,
) -> Result<(), NovelError> {
    let urls = noveler.get_chapter_urls_sorted(document)?;
    let urls = noveler.append_urls_with_orders(urls);
    let urls = remove_url_with_exist_file(urls, dir);

    tokio::spawn(async move {
        for (order, url) in urls {
            if let Err(err) = tx
                .send(Task::new(order, url, pending.clone(), tx.clone()))
                .await
            {
                eprintln!("Failed to send task: {err}");
            }
        }
    });

    Ok(())
}

async fn save_chapter(chapter: Chapter, dir: &Path) -> Result<(), NovelError> {
    let path = dir.join(format!("{}.txt", chapter.order));

    tokio::fs::write(path, chapter.content()).await?;

    println!("{:>10} => {}", "Saved", chapter.order);
    Ok(())
}

async fn enqueue_next_page(
    current_order: String,
    next_url: Url,
    tx: &mpsc::Sender<Task>,
    pending: Arc<AtomicUsize>,
) {
    let next_task = Task::new(format!("{current_order}_n"), next_url, pending, tx.clone());

    if let Err(e) = tx.send(next_task).await {
        eprintln!("Failed to enqueue next page: {e}");
    }
}

fn create_client(
    headers: Option<header::HeaderMap>,
    cf_ua: Option<&String>,
) -> Result<Client, NovelError> {
    // 若有 CF clearance，UA 必須與瀏覽器一致；否則用預設 UA
    let user_agent = cf_ua.map_or(USER_AGENT, |v| v);

    let mut client_builder = Client::builder()
        .user_agent(user_agent)
        .timeout(Duration::from_secs(60 * 3));

    if let Some(h) = headers {
        client_builder = client_builder.default_headers(h).cookie_store(true);
    }
    let client = client_builder
        .timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()?;

    Ok(client)
}

pub(crate) async fn download_novel(
    noveler: Arc<dyn Noveler>,
    url_contents: &str,
    headers: Option<header::HeaderMap>,
    // Cloudflare 場景下需與取得 cookie 時的瀏覽器 UA 完全一致
    cf_ua: Option<&String>,
    dir: &Path,
    limit: usize,
    interval: Duration,
) -> Result<PathBuf, NovelError> {
    let client = create_client(headers, cf_ua)?;

    let html =
        get_html_and_fix_encoding(client.clone(), url_contents, noveler.need_encoding()).await?;
    let document = visdom::Vis::load(html)?;

    let book = noveler.get_book_info(&document)?;
    if book.author.is_empty() || book.name.is_empty() {
        return Err(NovelError::BlockedByCloudflare("Book Info".to_string()));
    }

    let dir = dir
        .join("temp")
        .join(noveler.to_string())
        .join(book.to_string());
    tokio::fs::create_dir_all(dir.as_path()).await?;

    let (tx, mut rx) = mpsc::channel::<Task>(32);
    let semaphore = Arc::new(Semaphore::new(limit));

    // pending 紀錄「尚未完成」的任務數（含正在執行的），用 AtomicUsize 跨 task 共享
    let pending = Arc::new(AtomicUsize::new(0));

    let mut visited: HashMap<String, TaskStatus> = HashMap::new();
    let mut join_set: JoinSet<Result<String, NovelError>> = JoinSet::new();

    enqueue_tasks(&noveler, &document, &dir, tx, pending.clone())?;

    println!("開始下載: {book} (併發限制: {limit}, 間隔: {interval:?})");

    loop {
        if rx.is_closed() && join_set.is_empty() {
            if visited
                .values()
                .any(|s| matches!(s, TaskStatus::Processing))
            {
                return Err(NovelError::IncompleteUrl);
            }

            return Ok(dir);
        }

        tokio::select! {
            Some(task) = rx.recv() => {
                if let Some(TaskStatus::Success) = visited.get(&task.order) {
                    continue;
                }

                visited.insert(task.order.clone(), TaskStatus::Processing);

                println!("{:>10} => {:<8}: {}", "Insert", task.order, task.url);

                let noveler = noveler.clone();
                let dir = dir.clone();
                let client = client.clone();
                let pending = pending.clone();
                let semaphore = semaphore.clone();

                join_set.spawn(async move {
                    let permit = semaphore.clone().acquire_owned().await.expect("acquire semaphore permit");

                    tokio::time::sleep(interval).await;

                    println!("{:>10} => {:<8}: {}", "Process", task.order, task.url);

                    let result = noveler.fetch_chapter(client, &task.order, task.url.clone()).await;
                    drop(permit);

                    match result {
                        Ok((chapter, next_page)) => {
                            let order = chapter.order.clone();

                            save_chapter(chapter, &dir).await?;

                            if let Some(next_url) = next_page {
                                enqueue_next_page(order.clone(), next_url, &task.tx, pending).await;
                            }

                            Ok(order)
                        }
                        Err(e) if e.is_retryable() => {
                            handle_retry(task, &e).await;
                            Err(e)
                        }
                        Err(e) =>  Err(e),
                    }
                });
            }

            Some(result) = join_set.join_next() => {
                match result {
                    Ok(Ok(order)) => {
                        visited.insert(order, TaskStatus::Success);
                    }
                    Ok(Err(e)) => {
                        eprintln!("Task error: {e}");
                    }
                    Err(join_error) => {
                        eprintln!("Async task failed: {join_error:?}");
                    }
                }
            }
        }
    }
}

async fn handle_retry(task: Task, e: &NovelError) {
    if task.retry_count < MAX_RETRIES {
        let delay = RETRY_BASE_DELAY_MS * 2u64.pow(task.retry_count);

        println!(
            "{:>10} => {:<8}: {}, retry {}/{} after {}ms",
            "Retry",
            task.order,
            e,
            task.retry_count + 1,
            MAX_RETRIES,
            delay
        );

        tokio::time::sleep(Duration::from_millis(delay)).await;

        let tx = task.tx.clone();
        if let Err(err) = tx.send(task.with_retry()).await {
            eprintln!("Failed to re-enqueue task: {err}");
        }
    } else {
        eprintln!(
            "{:>10} => {}: exceeded {} retries, giving up",
            "GiveUp", task.order, MAX_RETRIES
        );
    }
}

/// 將章節目錄中所有 .txt 合併成單一檔案
pub(crate) async fn combine_txt(dir: &Path) -> Result<(), NovelError> {
    let mut save_path = dir.to_path_buf();
    save_path.set_extension("txt");

    let mut entries = tokio::fs::read_dir(dir).await?;
    let mut paths: Vec<PathBuf> = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file() {
            paths.push(path);
        }
    }
    paths.sort_unstable();

    tokio::task::spawn_blocking(move || -> Result<(), NovelError> {
        let mut output = fs::File::create(&save_path)?;

        for path in &paths {
            let mut input = fs::File::open(path)?;
            io::copy(&mut input, &mut output)?;
            write!(&mut output, "\n\n")?;
            if let Some(name) = path.file_name() {
                println!("Appended content of file: {}", name.display());
            }
        }
        println!("done");
        Ok(())
    })
    .await
    .expect("spawn_blocking panicked")?;

    Ok(())
}

async fn get_html_and_fix_encoding<T: IntoUrl>(
    client: Client,
    url: T,
    need_encoding: Option<&'static encoding_rs::Encoding>,
) -> Result<String, NovelError> {
    let resp = client.get(url).send().await?;

    // 403 / 503 通常是 Cloudflare 攔截，給出明確錯誤而非靜默回傳空頁面
    let status = resp.status();
    if status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::SERVICE_UNAVAILABLE
    {
        return Err(NovelError::CloudflareBlock(status.as_u16()));
    }

    match need_encoding {
        None => Ok(resp.text().await?),
        Some(encoding) => {
            let body_bytes = resp.bytes().await?;
            let (decoded, _, _) = encoding.decode(&body_bytes);
            Ok(decoded.into_owned())
        }
    }
}

fn remove_url_with_exist_file(urls: Vec<(String, Url)>, dir: &Path) -> Vec<(String, Url)> {
    urls.into_iter()
        .filter(|(order, _)| !dir.join(file_name(order)).is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chardetng::EncodingDetector;
    use regex::Regex;
    use std::{
        env,
        sync::atomic::{AtomicI32, Ordering as AtomicOrdering},
    };
    use tempdir::TempDir;

    async fn guess_coding<T: IntoUrl>(url: T) -> (&'static encoding_rs::Encoding, bool) {
        let resp = reqwest::get(url).await.unwrap();
        let body_bytes = resp.bytes().await.unwrap();
        let mut detector = EncodingDetector::new();
        detector.feed(&body_bytes, true);
        detector.guess_assess(None, true)
    }

    #[ignore = "Used for detecting coding"]
    #[tokio::test]
    async fn test_guess_coding() {
        dbg!(guess_coding("https://www.qbtr.cc/tongren/3655.html").await);
    }

    #[ignore = "Used for check coding"]
    #[tokio::test]
    async fn test_check_coding() {
        let client = reqwest::Client::new();
        let document = get_html_and_fix_encoding(
            client,
            "https://www.qbtr.cc/tongren/3655.html",
            Some(encoding_rs::GBK),
        )
        .await
        .unwrap();
        dbg!(document);
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
                .map(|n| Url::parse(&format!("{}/{}", &self.host, n)).unwrap())
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
                Ok(Some(Url::parse(&format!(
                    "{}/next_page/{num}",
                    &self.host
                ))?))
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
    async fn test_enqueue_tasks() {
        let server = mockito::Server::new_async().await;
        let url = server.url();
        let fake: Arc<dyn Noveler> = Arc::new(FakeNoveler::new(url));
        let dir = TempDir::new("noveler_test_enqueue_tasks").unwrap();
        let (tx, mut rx) = mpsc::channel::<Task>(100);
        let contents: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/hjwzw/contents.html"
        ));
        let document = visdom::Vis::load(contents).unwrap();
        let pending = Arc::new(AtomicUsize::new(0));

        enqueue_tasks(&fake, &document, dir.path(), tx, pending.clone()).unwrap();

        let mut i = 0;
        while rx.recv().await.is_some() {
            i += 1;
        }
        assert_eq!(i, 10);
    }

    #[tokio::test]
    async fn test_save_chapter() {
        let dir = TempDir::new("noveler_test_save_chapter").unwrap();

        let chapter = Chapter {
            order: "order".to_string(),
            title: "title".to_string(),
            text: "text".to_string(),
        };
        save_chapter(chapter.clone(), dir.path()).await.unwrap();

        let file_path = dir.path().join(file_name(&chapter.order));
        assert!(file_path.is_file());
        assert_eq!(
            tokio::fs::read_to_string(&file_path).await.unwrap(),
            "title\n\ntext"
        );
    }

    #[tokio::test]
    async fn test_basic_noveler() {
        let server = mockito::Server::new_async().await;
        let url = server.url();
        let dir = TempDir::new("noveler_test_basic_noveler").unwrap();

        let chapter_dir = download_novel(
            Arc::new(FakeNoveler::new(url.clone())) as Arc<dyn Noveler>,
            url.as_str(),
            None,
            None,
            dir.path(),
            5,
            Duration::from_millis(0),
        )
        .await
        .unwrap();

        for n in 1..=10u32 {
            assert!(dir
                .path()
                .join(format!("temp/FakeNoveler/author_name/{n:05}.txt"))
                .exists());
            assert!(dir
                .path()
                .join(format!("temp/FakeNoveler/author_name/{n:05}_n.txt"))
                .exists());
        }
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("temp/FakeNoveler/author_name/00001.txt"))
                .await
                .unwrap(),
            "title_00001\n\ntext_process_00001"
        );

        combine_txt(&chapter_dir).await.unwrap();
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("temp/FakeNoveler/author_name.txt"))
                .await
                .unwrap(),
            r"title_00001

text_process_00001

title_00001_n

text_process_00001_n

title_00002

text_process_00002

title_00002_n

text_process_00002_n

title_00003

text_process_00003

title_00003_n

text_process_00003_n

title_00004

text_process_00004

title_00004_n

text_process_00004_n

title_00005

text_process_00005

title_00005_n

text_process_00005_n

title_00006

text_process_00006

title_00006_n

text_process_00006_n

title_00007

text_process_00007

title_00007_n

text_process_00007_n

title_00008

text_process_00008

title_00008_n

text_process_00008_n

title_00009

text_process_00009

title_00009_n

text_process_00009_n

title_00010

text_process_00010

title_00010_n

text_process_00010_n

"
        );

        dir.close().unwrap();
    }

    #[ignore = "Online Test sometimes with env cf_clearance for Cloudflare"]
    #[tokio::test]
    async fn test_czbooks() {
        let dir = TempDir::new("noveler_test_czbooks").unwrap();
        let url = "https://czbooks.net/n/uhemc";
        let noveler = Czbooks::new().expect("create Czbooks ok");
        let headers = option_env!("cf_clearance").map(|cf_clearance| {
            header::HeaderMap::from_iter([(
                header::COOKIE,
                header::HeaderValue::from_str(&format!("cf_clearance={cf_clearance}"))
                    .expect("create header value cf_clearance ok"),
            )])
        });
        let chapter_dir = download_novel(
            Arc::new(noveler) as Arc<dyn Noveler>,
            url,
            headers,
            None,
            dir.path(),
            1,
            Duration::from_millis(1000),
        )
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
        let chapter_dir = download_novel(
            Arc::new(noveler) as Arc<dyn Noveler>,
            url,
            None,
            None,
            dir.path(),
            1,
            Duration::from_millis(1000),
        )
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
        let chapter_dir = download_novel(
            Arc::new(noveler) as Arc<dyn Noveler>,
            url,
            None,
            None,
            dir.path(),
            1,
            Duration::from_millis(1000),
        )
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
        let chapter_dir = download_novel(
            Arc::new(noveler) as Arc<dyn Noveler>,
            url,
            None,
            None,
            dir.path(),
            1,
            Duration::from_millis(1000),
        )
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
        let chapter_dir = download_novel(
            Arc::new(noveler) as Arc<dyn Noveler>,
            url,
            None,
            None,
            dir.path(),
            1,
            Duration::from_millis(1000),
        )
        .await
        .expect("download ok");
        assert!(chapter_dir.ends_with("射手兇猛"), "dir = {chapter_dir:?}");
        combine_txt(&chapter_dir).await.expect("combine txt ok");
        dir.close().unwrap();
    }

    #[ignore = "Online Test with env cf_clearance for Cloudflare"]
    #[tokio::test]
    async fn test_uukanshu() {
        let dir = TempDir::new("noveler_test_uukanshu").unwrap();
        let url = "https://uukanshu.cc/book/20692/";
        let noveler = UUkanshu::new(url).expect("create UUkanshu ok");
        #[allow(clippy::option_env_unwrap)]
        let cf_clearance = option_env!("cf_clearance").expect("env cf_clearance");
        let headers = header::HeaderMap::from_iter([(
            header::COOKIE,
            header::HeaderValue::from_str(&format!("cf_clearance={cf_clearance}"))
                .expect("create header value cf_clearance ok"),
        )]);
        let chapter_dir = download_novel(
            Arc::new(noveler) as Arc<dyn Noveler>,
            url,
            Some(headers),
            None,
            dir.path(),
            1,
            Duration::from_millis(1000),
        )
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
