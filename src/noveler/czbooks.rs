/// 小說狂人 <https://czbooks.net/>
use super::{Book, Chapter, NovelError, Noveler};
use regex::Regex;
use std::fmt::{self, Display};
use url::Url;
use visdom::types::Elements;

const PATTERNS: [(&str, &str); 2] = [("\u{3000}", ""), ("\n\n", "\n")];

pub(crate) struct Czbooks {
    replacer: Vec<(Regex, &'static str)>,
}

impl Czbooks {
    pub(crate) fn new() -> Result<Self, NovelError> {
        let mut replacer = Vec::with_capacity(PATTERNS.len());
        for (pat, s) in PATTERNS {
            let regex = Regex::new(pat)?;
            replacer.push((regex, s));
        }
        Ok(Self { replacer })
    }
}

impl Display for Czbooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "小說狂人")
    }
}

impl Noveler for Czbooks {
    fn get_book_info(&self, document: &Elements) -> Result<Book, NovelError> {
        let name = document
            .find(r"span.title")
            .text()
            .replace(['《', '》'], "");
        let author = document.find(r"span.author > a").text();
        Ok(Book { name, author })
    }

    fn get_chapter_urls_sorted(&self, document: &Elements) -> Result<Vec<Url>, NovelError> {
        document
            .find(r"ul.nav.chapter-list > li > a")
            .into_iter()
            .map(|x| {
                x.get_attribute("href")
                    .map(|attr| attr.to_string())
                    .ok_or(NovelError::NotFound("href".to_string()))
            })
            .map(|x| {
                x.and_then(|url_str| {
                    Url::parse(&format!("https:{url_str}")).map_err(NovelError::ParseError)
                })
            })
            .collect()
    }

    fn get_chapter(&self, document: &Elements, order: &str) -> Result<Chapter, NovelError> {
        let title = document.find(r"div.name").text().trim().to_string();
        if title.is_empty() {
            return Err(NovelError::BlockedByCloudflare("Title".to_string()));
        }

        let text = document.find(r"div.content").text();
        if text.is_empty() {
            return Err(NovelError::BlockedByCloudflare("Text".to_string()));
        }

        Ok(Chapter {
            order: order.to_string(),
            title,
            text,
        })
    }

    fn get_next_page(&self, _document: &Elements) -> Result<Option<Url>, NovelError> {
        Ok(None)
    }

    fn process_chapter(&self, chapter: Chapter) -> Chapter {
        let mut text = chapter.text;
        for (re, s) in &self.replacer {
            text = re.replace_all(&text, *s).to_string();
        }
        Chapter { text, ..chapter }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static CONTENTS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/czbooks/contents.html"
    ));
    static CONTENTS2: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/czbooks/contents2.html"
    ));
    static CHAPTER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/czbooks/chapter.html"
    ));
    static CHAPTER2: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/czbooks/chapter2.html"
    ));

    #[test]
    fn test_get_book_info() {
        let document = visdom::Vis::load(CONTENTS).unwrap();
        let novel = Czbooks::new().unwrap();
        let book = novel.get_book_info(&document).unwrap();
        assert_eq!(
            book,
            Book {
                name: "射手凶猛".to_string(),
                author: "初四兮".to_string()
            }
        );
    }

    #[test]
    fn test_get_book_info2() {
        let document = visdom::Vis::load(CONTENTS2).unwrap();
        let novel = Czbooks::new().unwrap();
        let book = novel.get_book_info(&document).unwrap();
        assert_eq!(
            book,
            Book {
                name: "這個世界過於危險".to_string(),
                author: "十裡桃花".to_string()
            }
        );
    }

    #[test]
    fn test_get_chapter_urls_sorted() {
        let document = visdom::Vis::load(CONTENTS).unwrap();
        let novel = Czbooks::new().unwrap();
        let urls = novel.get_chapter_urls_sorted(&document).unwrap();
        assert_eq!(
            urls.first().unwrap(),
            &Url::parse("https://czbooks.net/n/uilla7/und20").unwrap()
        );
        assert_eq!(
            urls.last().unwrap(),
            &Url::parse("https://czbooks.net/n/uilla7/ui5kpm").unwrap()
        );
    }

    #[test]
    fn test_get_chapter_urls_sorted2() {
        let document = visdom::Vis::load(CONTENTS2).unwrap();
        let novel = Czbooks::new().unwrap();
        let urls = novel.get_chapter_urls_sorted(&document).unwrap();
        assert_eq!(
            urls.first().unwrap(),
            &Url::parse("https://czbooks.net/n/uhemc/u7k9").unwrap()
        );
        assert_eq!(
            urls.last().unwrap(),
            &Url::parse("https://czbooks.net/n/uhemc/ud9fn").unwrap()
        );
    }

    #[test]
    fn test_get_chapter_content() {
        let document = visdom::Vis::load(CHAPTER).unwrap();
        let novel = Czbooks::new().unwrap();
        let chapter = novel.get_chapter(&document, "1").unwrap();
        assert_eq!(chapter.order, "1");
        assert_eq!(chapter.title, "《射手凶猛》第1章 老地方");
        let chapter = novel.process_chapter(chapter);
        assert!(chapter.text.starts_with("六月的首都日漸炎熱"));
        assert!(chapter.text.ends_with("“開個機子。”"));
    }

    #[test]
    fn test_get_chapter_content2() {
        let document = visdom::Vis::load(CHAPTER2).unwrap();
        let novel = Czbooks::new().unwrap();
        let chapter = novel.get_chapter(&document, "1").unwrap();
        assert_eq!(chapter.order, "1");
        assert_eq!(chapter.title, "《這個世界過於危險》第11章：暴食（上）");
        let chapter = novel.process_chapter(chapter);
        assert!(chapter.text.starts_with("11月5日，周一。"));
        assert!(chapter
            .text
            .ends_with("“你回來的正好，想吃漢堡了，你去幫我買吧！”"));
    }

    #[test]
    fn test_get_next_page() {
        let document = visdom::Vis::load(CHAPTER).unwrap();
        let novel = Czbooks::new().unwrap();
        assert_eq!(novel.get_next_page(&document).unwrap(), None);
    }
}
