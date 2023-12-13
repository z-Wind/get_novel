use async_trait::async_trait;
use reqwest::{Client, IntoUrl};
use std::collections::HashSet;
use std::fmt::Display;
use std::io::Write;
use std::panic;
use std::sync::Arc;
use std::time::Duration;
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;
use url::Url;
use visdom::types::Elements;

mod czbooks;
mod hjwzw;
mod novel543;
mod piaotia;
mod qbtr;
mod uukanshu;

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
    #[error("reqwest fail {0}")]
    ReqwestError(#[from] reqwest::Error),
    #[error("std io fail {0}")]
    StdIOError(#[from] io::Error),
    #[error("visdom fail {0}")]
    VisdomError(#[from] visdom::types::BoxDynError),
    #[error("AhoCorasick fail {0}")]
    AhoCorasickError(#[from] aho_corasick::BuildError),
    #[error("Regex fail {0}")]
    RegexError(#[from] regex::Error),
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

#[async_trait]
pub(crate) trait Noveler: Display {
    fn need_encoding(&self) -> Option<&'static encoding_rs::Encoding> {
        None
    }

    async fn process_url(
        &self,
        client: Client,
        order: &str,
        url: Url,
    ) -> Result<(Chapter, Option<Url>), NovelError> {
        let document = get_html_and_fix_encoding(client, url, self.need_encoding()).await?;
        let document = visdom::Vis::load(document)?;

        let mut chapter: Chapter = self.get_chapter(&document, order)?;
        chapter = self.process_chapter(chapter);

        let next_page = self.get_next_page(&document)?;

        Ok((chapter, next_page))
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
}

fn file_name(order: &str) -> String {
    format!("{order}.txt")
}

fn process_url_contents<'a, T>(
    noveler: &Arc<T>,
    document: &'a Elements<'a>,
    dir: &Path,
    tx: mpsc::Sender<(String, Url)>,
) -> Result<i32, NovelError>
where
    T: Noveler + std::marker::Sync + std::marker::Send + 'static,
{
    let urls = noveler.get_chapter_urls_sorted(document)?;
    let mut urls = noveler.append_urls_with_orders(urls);
    urls = remove_url_with_exist_file(urls, dir);

    let tasks = i32::try_from(urls.len()).expect("usize to i32 ok");
    tokio::spawn(async move {
        for url in urls {
            if let Err(err) = tx.send(url).await {
                eprintln!("Failed to send url: {err}");
            }
        }
    });

    Ok(tasks)
}

async fn process_save_task(
    chapter: Chapter,
    next_page: Option<Url>,
    dir: &Path,
    tx: mpsc::Sender<(String, Url)>,
) -> Result<i32, NovelError> {
    tokio::fs::write(dir.join(file_name(&chapter.order)), chapter.content()).await?;

    println!("{:>10} => {:<8}", "Done", chapter.order);

    let mut tasks_done = -1;
    if let Some(next_page_url) = next_page {
        tasks_done += 1;
        tokio::spawn(async move {
            let url = (chapter.order + "_n", next_page_url);
            if let Err(err) = tx.send(url).await {
                eprintln!("Failed to send url: {err}");
            }
        });
    }

    Ok(tasks_done)
}

pub(crate) async fn download_novel<'a, T>(
    noveler: Arc<T>,
    url_contents: &'a str,
    dir: &Path,
    limit: usize,
) -> Result<PathBuf, NovelError>
where
    T: Noveler + std::marker::Sync + std::marker::Send + 'static,
{
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60 * 3))
        .build()?;

    let document =
        get_html_and_fix_encoding(client.clone(), url_contents, noveler.need_encoding()).await?;
    // fs::write("test.html", document.html()).unwrap();
    let document = visdom::Vis::load(document)?;

    let book = noveler.get_book_info(&document)?;

    let dir = dir
        .join("temp")
        .join(noveler.to_string())
        .join(book.to_string());
    tokio::fs::create_dir_all(dir.as_path()).await?;

    let semaphore = Arc::new(Semaphore::new(limit)); // Adjust the concurrency limit as needed
    let (tx, mut rx) = mpsc::channel::<(String, Url)>(10);

    let mut set = HashSet::new();
    let mut tasks = process_url_contents(&noveler, &document, &dir, tx.clone())?;
    let mut join_set: JoinSet<Result<i32, NovelError>> = JoinSet::new();
    while tasks > 0 {
        tokio::select! {
            Some((order, url)) = rx.recv() => {
                if set.contains(&url) {
                    join_set.spawn(async move {
                        Ok(-1)
                    });
                    continue;
                }
                set.insert(url.clone());

                println!("{:>10} => {order:<8}: {url}", "Insert");

                let tx_c = tx.clone();
                let noveler_c = noveler.clone();
                let dir_c = dir.clone();
                let client = client.clone();
                let permit = semaphore.clone().acquire_owned().await.expect("acquire semaphore permit");
                join_set.spawn(async move {
                    println!("{:>10} => {order:<8}: {url}", "Process");
                    let (chapter, next_page) = match noveler_c.process_url(client, &order, url.clone()).await {
                        Ok(result) => result,
                        Err(NovelError::ReqwestError(e)) => {
                            if e.is_timeout() {
                                println!("{:>10} => {order:<8}: {url}", "TOutRedo");
                                if let Err(err) = tx_c.send((order, url)).await {
                                    eprintln!("Failed to send url: {err}");
                                }
                                return Ok(0);
                            }

                            return Err(e.into());
                        }
                        Err(e) => {
                            return Err(e);
                        },
                    };

                    // Release the semaphore permit
                    drop(permit);
                    process_save_task(chapter, next_page, &dir_c, tx_c).await
                });
            }
            Some(result) = join_set.join_next() => {
                match result {
                    Ok(result) => {
                        tasks += result?;
                        println!("{:<10} => {tasks:05}", "Tasks");
                    }
                    Err(join_error) => {
                        eprintln!("Async task failed: {join_error:?}");
                        if join_error.is_panic() {
                            eprintln!("Async task panicked!");
                        }
                    }
                }
            }
        };
    }

    Ok(dir)
}

pub(crate) fn combine_txt(dir: &Path) -> Result<(), NovelError> {
    let mut save_path = dir.to_path_buf();
    save_path.set_extension("txt");

    let mut output = fs::File::create(save_path)?;

    let entries: Vec<fs::DirEntry> = dir.read_dir()?.collect::<Result<_, std::io::Error>>()?;
    let mut paths: Vec<PathBuf> = entries.into_iter().map(|entry| entry.path()).collect();
    paths.sort_unstable();
    for path in paths {
        let mut input = fs::File::open(&path)?;
        io::copy(&mut input, &mut output)?;

        // Add a line break after copying each file
        write!(&mut output, "\n\n")?;

        if let Some(file_name) = path.file_name() {
            println!("Appended content of file: {file_name:?}");
        }
    }

    println!("done");
    Ok(())
}

async fn get_html_and_fix_encoding<T: IntoUrl>(
    client: Client,
    url: T,
    need_encoding: Option<&'static encoding_rs::Encoding>,
) -> Result<String, NovelError> {
    let resp = client.get(url).send().await?;

    match need_encoding {
        None => Ok(resp.text().await?),
        Some(encoding) => {
            // Extract raw body bytes
            let body_bytes = resp.bytes().await?;

            // Decode the response body to UTF-8 using the encoding
            let (decoded, _, _) = encoding.decode(&body_bytes);

            // Parse the decoded HTML back into a scraper::Html
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
    use async_trait::async_trait;
    use chardetng::EncodingDetector;
    use regex::Regex;
    use std::sync::atomic::{AtomicI32, Ordering};
    use tempdir::TempDir;

    async fn guess_coding<T: IntoUrl>(url: T) -> (&'static encoding_rs::Encoding, bool) {
        let resp = reqwest::get(url).await.unwrap();

        // Extract raw body bytes
        let body_bytes = resp.bytes().await.unwrap();

        // Use chardetng to detect encoding
        let mut detector = EncodingDetector::new();
        detector.feed(&body_bytes, true);
        detector.guess_assess(None, true)
    }

    #[ignore = "only for detecting coding"]
    #[tokio::test]
    async fn test_guess_coding() {
        dbg!(guess_coding("https://www.qbtr.cc/tongren/3655.html").await);
    }

    #[ignore = "only for check coding"]
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

    #[async_trait]
    impl Noveler for FakeNoveler {
        fn get_book_info(&self, _document: &Elements) -> Result<Book, NovelError> {
            let name = "name".to_string();
            let author = "author".to_string();
            Ok(Book { name, author })
        }

        fn get_chapter_urls_sorted(&self, _document: &Elements) -> Result<Vec<Url>, NovelError> {
            Ok((1..)
                .take(10)
                .map(|n| Url::parse(&format!("{}/{}", &self.host, n)).unwrap())
                .collect())
        }

        fn get_chapter(&self, _document: &Elements, order: &str) -> Result<Chapter, NovelError> {
            let title = format!("title_{order}");
            let text = format!("text_{order}");
            let order = order.to_string();
            Ok(Chapter { order, title, text })
        }

        fn get_next_page(&self, _document: &Elements) -> Result<Option<Url>, NovelError> {
            let num = self.num.fetch_add(1, Ordering::SeqCst);

            if num > 10 {
                Ok(None)
            } else {
                let url = Url::parse(&format!("{}/next_page/{num}", &self.host))?;
                Ok(Some(url))
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
    async fn test_process_url_contents() {
        // Request a new server from the pool
        let server = mockito::Server::new();

        // Use one of these addresses to configure your client
        let url = server.url();

        let fake = Arc::new(FakeNoveler::new(url));
        let dir = TempDir::new("noveler_test_process_url_contents").unwrap();
        let path = dir.path();
        let (tx, _) = mpsc::channel::<(String, Url)>(5);

        let contents: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/hjwzw/contents.html"
        ));
        let document = visdom::Vis::load(contents).unwrap();

        let result = process_url_contents(&fake, &document, path, tx).unwrap();
        assert_eq!(result, 10);
    }

    #[tokio::test]
    async fn test_process_save_task() {
        let dir = TempDir::new("noveler_test_process_save_task").unwrap();
        let path = dir.path();

        let (tx, _) = mpsc::channel::<(String, Url)>(5);

        let chapter = Chapter {
            order: "order".to_string(),
            title: "title".to_string(),
            text: "text".to_string(),
        };
        process_save_task(chapter.clone(), None, path, tx)
            .await
            .unwrap();

        let file_path = path.join(file_name(&chapter.order));
        dbg!(&file_path);
        assert!(file_path.is_file());
        assert_eq!(fs::read_to_string(file_path).unwrap(), "title\n\ntext");
    }

    #[tokio::test]
    async fn test_basic_noveler() {
        // Request a new server from the pool
        let server = mockito::Server::new();

        // Use one of these addresses to configure your client
        let url = server.url();

        let fake = FakeNoveler::new(url.clone());
        let dir = TempDir::new("noveler_test_basic_noveler").unwrap();
        let path = dir.path();
        let chapter_dir = download_novel(Arc::new(fake), url.as_str(), path, 5)
            .await
            .unwrap();

        assert!(path.join("temp/FakeNoveler/author_name/00001.txt").exists());
        assert!(path
            .join("temp/FakeNoveler/author_name/00001_n.txt")
            .exists());
        assert!(path.join("temp/FakeNoveler/author_name/00002.txt").exists());
        assert!(path.join("temp/FakeNoveler/author_name/00003.txt").exists());
        assert!(path.join("temp/FakeNoveler/author_name/00004.txt").exists());
        assert!(path.join("temp/FakeNoveler/author_name/00005.txt").exists());
        assert!(path.join("temp/FakeNoveler/author_name/00006.txt").exists());
        assert!(path.join("temp/FakeNoveler/author_name/00007.txt").exists());
        assert!(path.join("temp/FakeNoveler/author_name/00008.txt").exists());
        assert!(path.join("temp/FakeNoveler/author_name/00009.txt").exists());
        assert!(path.join("temp/FakeNoveler/author_name/00010.txt").exists());
        assert_eq!(
            tokio::fs::read_to_string(path.join("temp/FakeNoveler/author_name/00001.txt"))
                .await
                .unwrap(),
            "title_00001\n\ntext_process_00001"
        );

        combine_txt(&chapter_dir).unwrap();
        assert_eq!(
            tokio::fs::read_to_string(path.join("temp/FakeNoveler/author_name.txt"))
                .await
                .unwrap(),
            r#"title_00001

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

"#
        );

        dir.close().unwrap();
    }

    #[ignore = "online test"]
    #[tokio::test]
    async fn test_novel543() {
        let dir = TempDir::new("noveler_test_novel543").unwrap();
        let path = dir.path();

        let url = "https://www.novel543.com/0413188175/dir";
        let noveler = Novel543::new(url).expect("create Novel543 ok");

        let chapter_dir = download_novel(Arc::new(noveler), url, path, 1)
            .await
            .expect("download ok");

        combine_txt(&chapter_dir).expect("combine txt ok");

        dir.close().unwrap();
    }

    #[ignore = "online test"]
    #[tokio::test]
    async fn test_hjwzw() {
        let dir = TempDir::new("noveler_test_hjwzw").unwrap();
        let path = dir.path();

        let url = "https://tw.hjwzw.com/Book/Chapter/48386";
        let noveler = Hjwzw::new(url).expect("create Hjwzw ok");

        let chapter_dir = download_novel(Arc::new(noveler), url, path, 10)
            .await
            .expect("download ok");

        combine_txt(&chapter_dir).expect("combine txt ok");

        dir.close().unwrap();
    }

    #[ignore = "online test"]
    #[tokio::test]
    async fn test_piaotia() {
        let dir = TempDir::new("noveler_test_piaotia").unwrap();
        let path = dir.path();

        let url = "https://www.piaotia.com/html/14/14881/";
        let noveler = Piaotia::new(url).expect("create Piaotia ok");

        let chapter_dir = download_novel(Arc::new(noveler), url, path, 10)
            .await
            .expect("download ok");

        combine_txt(&chapter_dir).expect("combine txt ok");

        dir.close().unwrap();
    }

    #[ignore = "online test"]
    #[tokio::test]
    async fn test_uukanshu() {
        let dir = TempDir::new("noveler_test_uukanshu").unwrap();
        let path = dir.path();

        let url = "https://tw.uukanshu.com/b/239329/";
        let noveler: UUkanshu = UUkanshu::new(url).expect("create UUkanshu ok");

        let chapter_dir = download_novel(Arc::new(noveler), url, path, 10)
            .await
            .expect("download ok");

        combine_txt(&chapter_dir).expect("combine txt ok");

        dir.close().unwrap();
    }

    #[ignore = "only for compare"]
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
            let duration = start.elapsed();
            println!("nipper {duration:?}");
        }

        {
            let start = std::time::Instant::now();
            for _ in 0..n {
                let document = scraper::Html::parse_document(html);
                let selector = scraper::Selector::parse(selector).unwrap();
                let a = document
                    .select(&selector)
                    .next()
                    .unwrap()
                    .value()
                    .attr("href")
                    .unwrap();
                assert_eq!(a, "//czbooks.net/n/uilla7/und20");
            }
            let duration = start.elapsed();
            println!("scraper {duration:?}");
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
            let duration = start.elapsed();
            println!("visdom {duration:?}");
        }

        {
            let start = std::time::Instant::now();
            for _ in 0..n {
                let document = accessibility_scraper::Html::parse_document(html);
                let selector = accessibility_scraper::Selector::parse(selector).unwrap();
                let a = document
                    .select(&selector)
                    .next()
                    .unwrap()
                    .value()
                    .attr("href")
                    .unwrap();
                assert_eq!(a, "//czbooks.net/n/uilla7/und20");
            }
            let duration = start.elapsed();
            println!("accessibility-scraper {duration:?}");
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
            let duration = start.elapsed();
            println!("crabquery {duration:?}");
        }
    }
}
