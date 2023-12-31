/// 飄天 <https://www.piaotia.com/>
use super::{Book, Chapter, NovelError, Noveler};
use regex::Regex;
use std::fmt::{self, Display};
use url::Url;
use visdom::types::Elements;

pub(crate) struct Piaotia {
    base: Url,
    replacer: (Vec<Regex>, Vec<String>),
}

impl Piaotia {
    pub(crate) fn new(url: &str) -> Result<Self, NovelError> {
        let base = Url::parse(url)?;

        let patterns = ["(?s)（快捷键 ←）.*", "(?s).*返回书页"];
        let replace_with = ["", ""]
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

impl Display for Piaotia {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "飄天")
    }
}

impl Noveler for Piaotia {
    fn need_encoding(&self) -> Option<&'static encoding_rs::Encoding> {
        Some(encoding_rs::GBK)
    }

    fn get_book_info(&self, document: &Elements) -> Result<Book, NovelError> {
        let selector = r"div.title h1";
        let name = document.find(selector).text().replace("最新章节", "");

        let selector = r"meta[name=author]";
        let author = document
            .find(selector)
            .attr("content")
            .ok_or(NovelError::NotFound("author content".to_string()))?
            .to_string();
        Ok(Book { name, author })
    }

    fn get_chapter_urls_sorted(&self, document: &Elements) -> Result<Vec<Url>, NovelError> {
        let selector = r"div.centent li a";
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
        let selector = r"H1";
        let title = document
            .find(selector)
            .text()
            .trim()
            .replace("射手凶猛 ", "")
            .to_string();

        let selector = r"html";
        let text: String = document.find(selector).text();

        let order = order.to_string();
        Ok(Chapter { order, title, text })
    }

    fn get_next_page(&self, _document: &Elements) -> Result<Option<Url>, NovelError> {
        Ok(None)
    }

    fn process_chapter(&self, chapter: Chapter) -> Chapter {
        let mut text = chapter.text;
        for (re, s) in self.replacer.0.iter().zip(self.replacer.1.iter()) {
            text = re.replace_all(&text, s).to_string();
        }

        text = text
            .split(['\n', '\u{a0}'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<&str>>()
            .join("\n");

        Chapter { text, ..chapter }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static CONTENTS: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/piaotia/contents.html"
    ));
    static CHAPTER: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/piaotia/chapter.html"
    ));

    #[test]
    fn test_get_book_info() {
        let novel = Piaotia::new("https://www.piaotia.com/html/14/14881/").unwrap();
        let (html, _, _) = novel.need_encoding().unwrap().decode(CONTENTS);
        let document = visdom::Vis::load(html).unwrap();
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
        let novel = Piaotia::new("https://www.piaotia.com/html/14/14881/").unwrap();
        let (html, _, _) = novel.need_encoding().unwrap().decode(CONTENTS);
        let document = visdom::Vis::load(html).unwrap();
        let urls = novel.get_chapter_urls_sorted(&document).unwrap();
        assert_eq!(
            urls.first().unwrap(),
            &Url::parse("https://www.piaotia.com/html/14/14881/9983851.html").unwrap()
        );
        assert_eq!(
            urls.last().unwrap(),
            &Url::parse("https://www.piaotia.com/html/14/14881/10573157.html").unwrap()
        );
    }

    #[test]
    fn test_get_chapter_content() {
        let novel = Piaotia::new("https://www.piaotia.com/html/14/14881/").unwrap();
        let (html, _, _) = novel.need_encoding().unwrap().decode(CHAPTER);
        let document = visdom::Vis::load(html).unwrap();
        let chapter = novel.get_chapter(&document, "1").unwrap();
        assert_eq!(chapter.order, "1".to_string());
        assert_eq!(chapter.title, "第一章 老地方".to_string());
        let chapter = novel.process_chapter(chapter);
        dbg!(&chapter.text);
        assert!(chapter.text.starts_with("六月的首都日渐炎热。"));
        assert!(chapter.text.ends_with("“开个机子。”"));
    }

    #[test]
    fn test_get_next_page() {
        let novel = Piaotia::new("https://www.piaotia.com/html/14/14881/").unwrap();
        let (html, _, _) = novel.need_encoding().unwrap().decode(CHAPTER);
        let document = visdom::Vis::load(html).unwrap();
        let url = novel.get_next_page(&document).unwrap();
        assert_eq!(url, None);
    }
}
