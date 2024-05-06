/// 全本同人 <https://www.qbtr.cc/>
use super::{Book, Chapter, NovelError, Noveler};
use regex::Regex;
use std::fmt::{self, Display};
use url::Url;
use visdom::types::Elements;

const PATTERNS: [(&str, &str); 0] = [];

pub(crate) struct Qbtr {
    base: Url,
    replacer: Vec<(Regex, &'static str)>,
}

impl Qbtr {
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

        let mut replacer = Vec::with_capacity(PATTERNS.len());
        for (pat, s) in PATTERNS {
            let regex = Regex::new(pat)?;
            replacer.push((regex, s));
        }

        Ok(Self { base, replacer })
    }
}

impl Display for Qbtr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "全本同人")
    }
}

impl Noveler for Qbtr {
    fn need_encoding(&self) -> Option<&'static encoding_rs::Encoding> {
        Some(encoding_rs::GBK)
    }

    fn get_book_info(&self, document: &Elements) -> Result<Book, NovelError> {
        let selector = r"div.infos > h1";
        let name = document.find(selector).text();

        let selector = r"div.date > span";
        let author = document.find(selector).text().replace("作者：", "");
        Ok(Book { name, author })
    }

    fn get_chapter_urls_sorted(&self, document: &Elements) -> Result<Vec<Url>, NovelError> {
        let selector = r"div.book_list.clearfix > ul > li > a";
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
        let selector = r"div.read_chapterName.tc > h1";
        let title = document.find(selector).text().trim().to_string();

        let selector = r"div.read_chapterDetail > p";
        let text: String = document
            .find(selector)
            .into_iter()
            .map(|x| x.text().trim().to_string())
            .collect::<Vec<_>>()
            .join("\n");

        let order = order.to_string();
        Ok(Chapter { order, title, text })
    }

    fn get_next_page(&self, _document: &Elements) -> Result<Option<Url>, NovelError> {
        Ok(None)
    }

    fn process_chapter(&self, chapter: Chapter) -> Chapter {
        let mut text = chapter.text;
        for (re, s) in &self.replacer {
            text = re.replace_all(&text, *s).to_string();
        }

        text = text
            .split(['\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .skip(2)
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
        "/tests/qbtr/contents.html"
    ));
    static CHAPTER: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/qbtr/chapter.html"
    ));

    #[test]
    fn test_get_book_info() {
        let novel = Qbtr::new("https://www.qbtr.cc/tongren/3655.html").unwrap();
        let (html, _, _) = novel.need_encoding().unwrap().decode(CONTENTS);
        let document = visdom::Vis::load(html).unwrap();
        let book = novel.get_book_info(&document).unwrap();
        assert_eq!(
            book,
            Book {
                name: "我的大宝剑".to_string(),
                author: "学霸殿下".to_string()
            }
        );
    }

    #[test]
    fn test_get_chapter_urls_sorted() {
        let novel = Qbtr::new("https://www.qbtr.cc/tongren/3655.html").unwrap();
        let (html, _, _) = novel.need_encoding().unwrap().decode(CONTENTS);
        let document = visdom::Vis::load(html).unwrap();
        let urls = novel.get_chapter_urls_sorted(&document).unwrap();
        assert_eq!(
            urls.first().unwrap(),
            &Url::parse("https://www.qbtr.cc/tongren/3655/1.html").unwrap()
        );
        assert_eq!(
            urls.last().unwrap(),
            &Url::parse("https://www.qbtr.cc/tongren/3655/1807.html").unwrap()
        );
    }

    #[test]
    fn test_get_chapter_content() {
        let novel = Qbtr::new("https://www.qbtr.cc/tongren/3655.html").unwrap();
        let (html, _, _) = novel.need_encoding().unwrap().decode(CHAPTER);
        let document = visdom::Vis::load(html).unwrap();
        let chapter = novel.get_chapter(&document, "1").unwrap();
        assert_eq!(chapter.order, "1".to_string());
        assert_eq!(chapter.title, "我的大宝剑 第1章".to_string());
        assert!(!chapter.text.is_empty());
        let chapter = novel.process_chapter(chapter);
        dbg!(&chapter.text);
        assert!(chapter.text.starts_with("始皇历1838年，天元战争结束"));
        assert!(chapter.text.ends_with("充满了幸福和快乐。"));
    }

    #[test]
    fn test_get_next_page() {
        let novel = Qbtr::new("https://www.qbtr.cc/tongren/3655.html").unwrap();
        let (html, _, _) = novel.need_encoding().unwrap().decode(CHAPTER);
        let document = visdom::Vis::load(html).unwrap();
        let url = novel.get_next_page(&document).unwrap();
        assert_eq!(url, None);
    }
}
