/// 小說狂人 <https://czbooks.net/>
use super::{Book, Chapter, NovelError, Noveler};
use regex::Regex;
use std::fmt::{self, Display};
use url::Url;
use visdom::types::Elements;

const PATTERNS: [(&str, &str); 4] = [
    ("\u{3000}", ""),
    ("\u{2003}", ""),
    ("\r\n", "\n"),
    ("\n\n", "\n"),
];

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
        let name = document.find(r"h1.title").text().replace(['《', '》'], "");
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
        let title = document.find(r"h1.name").text().trim().to_string();
        if title.is_empty() {
            return Err(NovelError::NotFound("Chapter Title".to_string()));
        }

        let text = document.find(r"div.content").text();
        if text.is_empty() {
            return Err(NovelError::NotFound("Chapter Text".to_string()));
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
    static CHAPTER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/czbooks/chapter.html"
    ));

    #[test]
    fn test_get_book_info() {
        let document = visdom::Vis::load(CONTENTS).unwrap();
        let novel = Czbooks::new().unwrap();
        let book = novel.get_book_info(&document).unwrap();
        assert_eq!(
            book,
            Book {
                name: "謝家的短命鬼長命百歲了".to_string(),
                author: "怡然".to_string()
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
            &Url::parse("https://czbooks.net/n/s66fh9/s6pm5p1a?chapterNumber=0").unwrap()
        );
        assert_eq!(
            urls.last().unwrap(),
            &Url::parse("https://czbooks.net/n/s66fh9/s6peg4hm?chapterNumber=949").unwrap()
        );
    }

    #[test]
    fn test_get_chapter_content() {
        let document = visdom::Vis::load(CHAPTER).unwrap();
        let novel = Czbooks::new().unwrap();
        let chapter = novel.get_chapter(&document, "1").unwrap();
        assert_eq!(chapter.order, "1");
        assert_eq!(chapter.title, "《謝家的短命鬼長命百歲了》第93章 相識");
        let chapter = novel.process_chapter(chapter);
        assert!(chapter.text.starts_with("第93章 相識"));
        assert!(chapter.text.ends_with("(本章完)"));
    }

    #[test]
    fn test_get_next_page() {
        let document = visdom::Vis::load(CHAPTER).unwrap();
        let novel = Czbooks::new().unwrap();
        assert_eq!(novel.get_next_page(&document).unwrap(), None);
    }
}
