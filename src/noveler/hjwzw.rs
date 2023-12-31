/// 黃金屋 <https://tw.hjwzw.com/>
use super::{Book, Chapter, NovelError, Noveler};
use regex::Regex;
use std::fmt::{self, Display};
use url::Url;
use visdom::types::Elements;

pub(crate) struct Hjwzw {
    base: Url,
    replacer: (Vec<Regex>, Vec<String>),
}

impl Hjwzw {
    pub(crate) fn new(url: &str) -> Result<Self, NovelError> {
        let mut base = Url::parse(url)?;

        match base.path_segments_mut() {
            Ok(mut path) => {
                path.clear();
            }
            Err(()) => {
                return Err(NovelError::CannotBeABase(url.to_string()));
            }
        }
        base.set_query(None);

        let patterns = [""];
        let replace_with = [""]
            .into_iter()
            .map(std::string::ToString::to_string)
            .collect();
        let regexes = patterns
            .into_iter()
            .map(Regex::new)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            base,
            replacer: (regexes, replace_with),
        })
    }
}

impl Display for Hjwzw {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "黃金屋")
    }
}

impl Noveler for Hjwzw {
    fn get_book_info(&self, document: &Elements) -> Result<Book, NovelError> {
        let selector = r"h1";
        let name = document.find(selector).text();

        let selector =
            r"body > div:first-child > table:nth-of-type(7) tr:nth-child(2) a:first-child";
        let author = document.find(selector).text().replace("作者 / ", "");
        Ok(Book { name, author })
    }

    fn get_chapter_urls_sorted(&self, document: &Elements) -> Result<Vec<Url>, NovelError> {
        let selector = r"div#tbchapterlist a";
        document
            .find(selector)
            .into_iter()
            .map(|x| {
                x.get_attribute("href")
                    .map(|attr| attr.to_string())
                    .ok_or(NovelError::NotFound("href".to_string()))
            })
            .map(|x| x.and_then(|url_str| self.base.join(&url_str).map_err(NovelError::ParseError)))
            .collect()
    }

    fn get_chapter(&self, document: &Elements, order: &str) -> Result<Chapter, NovelError> {
        let selector = r"table:nth-of-type(7) h1";
        let title = document.find(selector).text().trim().to_string();

        let doc = document.cloned();
        doc.find("div#Pan_Ad1").remove();
        let selector = r"table:nth-of-type(7) div:nth-of-type(4)";
        let text: String = doc.find(selector).text();

        let order = order.to_string();
        Ok(Chapter { order, title, text })
    }

    fn get_next_page(&self, _document: &Elements) -> Result<Option<Url>, NovelError> {
        Ok(None)
    }

    fn process_chapter(&self, chapter: Chapter) -> Chapter {
        let mut text = chapter.text;
        text = text
            .split(['\n', '\r'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .skip(2)
            .collect::<Vec<&str>>()
            .join("\n");
        for (re, s) in self.replacer.0.iter().zip(self.replacer.1.iter()) {
            text = re.replace_all(&text, s).to_string();
        }

        Chapter { text, ..chapter }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static CONTENTS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/hjwzw/contents.html"
    ));
    static CHAPTER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/hjwzw/chapter.html"
    ));
    static CHAPTER2: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/hjwzw/chapter2.html"
    ));

    #[test]
    fn test_get_book_info() {
        let html = CONTENTS;
        let document = visdom::Vis::load(html).unwrap();
        let novel = Hjwzw::new("https://tw.hjwzw.com/Book/Chapter/35728").unwrap();
        let book = novel.get_book_info(&document).unwrap();
        assert_eq!(
            book,
            Book {
                name: "修真聊天群".to_string(),
                author: "圣騎士的傳說".to_string()
            }
        );
    }

    #[test]
    fn test_get_chapter_urls_sorted() {
        let html = CONTENTS;
        let document = visdom::Vis::load(html).unwrap();
        let novel = Hjwzw::new("https://tw.hjwzw.com/Book/Chapter/35728").unwrap();
        let urls = novel.get_chapter_urls_sorted(&document).unwrap();
        assert_eq!(
            urls.first().unwrap(),
            &Url::parse("https://tw.hjwzw.com/Book/Read/35728,20025406").unwrap()
        );
        assert_eq!(
            urls.last().unwrap(),
            &Url::parse("https://tw.hjwzw.com/Book/Read/35728,18955259").unwrap()
        );
    }

    #[test]
    fn test_get_chapter_content() {
        let html = CHAPTER;
        let document = visdom::Vis::load(html).unwrap();
        let novel = Hjwzw::new("https://tw.hjwzw.com/Book/Chapter/35728").unwrap();
        let chapter = novel.get_chapter(&document, "1").unwrap();
        assert_eq!(chapter.order, "1".to_string());
        assert_eq!(chapter.title, "第一章 黃山真君和九洲一號群".to_string());
        let chapter = novel.process_chapter(chapter);
        dbg!(&chapter.text);
        assert!(chapter.text.starts_with("2019年5月20日，星期一。"));
        assert!(chapter.text.ends_with("是誤加嗎？"));
    }

    #[test]
    fn test_get_chapter2_content() {
        let html = CHAPTER2;
        let document = visdom::Vis::load(html).unwrap();
        let novel = Hjwzw::new("https://tw.hjwzw.com/Book/Chapter/35728").unwrap();
        let chapter = novel.get_chapter(&document, "1").unwrap();
        assert_eq!(chapter.order, "1".to_string());
        assert_eq!(chapter.title, "第二章 前進,榮譽15".to_string());
        let chapter = novel.process_chapter(chapter);
        dbg!(&chapter.text);
        assert!(chapter.text.starts_with("當期的特訓學員在經過考核"));
        assert!(chapter.text.ends_with("“你們他媽的倒是帶上我啊。”"));
    }

    #[test]
    fn test_get_next_page() {
        let html = CHAPTER;
        let document = visdom::Vis::load(html).unwrap();
        let novel = Hjwzw::new("https://tw.hjwzw.com/Book/Chapter/35728").unwrap();
        let url = novel.get_next_page(&document).unwrap();
        assert_eq!(url, None);
    }
}
