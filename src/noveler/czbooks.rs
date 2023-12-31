/// 小說狂人 <https://czbooks.net/>
use super::{Book, Chapter, NovelError, Noveler};
use aho_corasick::AhoCorasick;
use std::fmt::{self, Display};
use url::Url;
use visdom::types::Elements;

pub(crate) struct Czbooks {
    replacer: (AhoCorasick, Vec<String>),
}

impl Czbooks {
    pub(crate) fn new() -> Result<Self, NovelError> {
        let patterns = ["\u{3000}", "\n\n"];
        let replace_with = ["", "\n"]
            .into_iter()
            .map(std::string::ToString::to_string)
            .collect();

        let ac = AhoCorasick::new(patterns)?;

        Ok(Self {
            replacer: (ac, replace_with),
        })
    }
}

impl Display for Czbooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "小說狂人")
    }
}

impl Noveler for Czbooks {
    fn get_book_info(&self, document: &Elements) -> Result<Book, NovelError> {
        let selector = r"span.title";
        let name = document.find(selector).text().replace(['《', '》'], "");

        let selector = r"span.author > a";
        let author = document.find(selector).text();
        Ok(Book { name, author })
    }

    fn get_chapter_urls_sorted(&self, document: &Elements) -> Result<Vec<Url>, NovelError> {
        let selector = r"ul.nav.chapter-list > li > a";
        document
            .find(selector)
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
        let selector = r"div.name";
        let title = document
            .find(selector)
            .text()
            .trim()
            .replace("《射手凶猛》", "")
            .to_string();

        let selector = r"div.content";
        let text = document.find(selector).text();

        let order = order.to_string();
        Ok(Chapter { order, title, text })
    }

    fn get_next_page(&self, _document: &Elements) -> Result<Option<Url>, NovelError> {
        Ok(None)
    }

    fn process_chapter(&self, mut chapter: Chapter) -> Chapter {
        chapter.text = self
            .replacer
            .0
            .replace_all(&chapter.text, &self.replacer.1)
            .to_string();
        chapter
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
        let html = CONTENTS;
        let document = visdom::Vis::load(html).unwrap();
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
    fn test_get_chapter_urls_sorted() {
        let html = CONTENTS;
        let document = visdom::Vis::load(html).unwrap();
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
    fn test_get_chapter_content() {
        let html = CHAPTER;
        let document = visdom::Vis::load(html).unwrap();
        let novel = Czbooks::new().unwrap();
        let chapter = novel.get_chapter(&document, "1").unwrap();
        assert_eq!(chapter.order, "1".to_string());
        assert_eq!(chapter.title, "第1章 老地方".to_string());
        let chapter = novel.process_chapter(chapter);
        dbg!(&chapter.text);
        assert!(chapter.text.starts_with("六月的首都日漸炎熱"));
        assert!(chapter.text.ends_with("“開個機子。”"));
    }

    #[test]
    fn test_get_next_page() {
        let html = CHAPTER;
        let document = visdom::Vis::load(html).unwrap();
        let novel = Czbooks::new().unwrap();
        let url = novel.get_next_page(&document).unwrap();
        assert_eq!(url, None);
    }
}
