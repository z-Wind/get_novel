/// UU看書 <https://uukanshu.cc/>
use super::{Book, Chapter, NovelError, Noveler};
use regex::Regex;
use std::fmt::{self, Display};
use url::Url;
use visdom::types::Elements;

const PATTERNS: [(&str, &str); 8] = [
    (r"(?s)如果喜歡.*，請把網址發給您的朋友。.*", ""),
    (r"(?s)如果喜欢.*，请把网址发给您的朋友。.*", ""),
    (
        r"[wｗ]{3}[．\.][ｕu][ｕu][ｋk][ａa][ｎn][ｓs][ｈh][ｕu][．\.][ｃc][ｏo][ｍm]",
        "",
    ),
    (
        r"[wｗ]{3}[．\.][ｕu][ｕu][ｋk][ａa][ｎn][ｓs][ｈh][ｕu][．\.][ｎn][ｅe][ｔt]",
        "",
    ),
    (r"[ｕuＵU]{2}看书[ ]*", ""),
    (r"[ｕuＵU]{2}看書[ ]*", ""),
    (r"請記住本書首發域名：。：", ""),
    (r"请记住本书首发域名：。：", ""),
];

pub(crate) struct UUkanshu {
    base: Url,
    replacer: Vec<(Regex, &'static str)>,
}

impl UUkanshu {
    pub(crate) fn new(url: &str) -> Result<Self, NovelError> {
        let mut base = Url::parse(url)?;

        match base.path_segments_mut() {
            Ok(mut path) => {
                path.clear();
            }
            Err(()) => return Err(NovelError::CannotBeABase(url.to_string())),
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

impl Display for UUkanshu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UU看書")
    }
}

impl Noveler for UUkanshu {
    fn get_book_info(&self, document: &Elements) -> Result<Book, NovelError> {
        let name = document.find(r"div.bookinfo > h1.booktitle").text();
        let author = document.find(r"div.bookinfo > p.booktag > a").text();
        Ok(Book { name, author })
    }

    fn get_chapter_urls_sorted(&self, document: &Elements) -> Result<Vec<Url>, NovelError> {
        document
            .find(r"div#list-chapterAll a")
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
        let title = document.find(r"div.book.read h1").text().trim().to_string();
        if title.is_empty() {
            return Err(NovelError::BlockedByCloudflare("Title".to_string()));
        }

        let text = document.find(r"div.readcotent").text();
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

        // 先按換行與全形空白切分
        text = text
            .split(['\n', '\u{3000}', '\u{a0}', '\r'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<&str>>()
            .join("\n");

        // 再按半形雙空格切分（部分廣告文字用雙空格分隔）
        text = text
            .split("  ")
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

    static CONTENTS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/uukanshu/contents.html"
    ));
    static CHAPTER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/uukanshu/chapter.html"
    ));

    #[test]
    fn test_get_book_info() {
        let document = visdom::Vis::load(CONTENTS).unwrap();
        let novel = UUkanshu::new("https://uukanshu.cc/book/20692/").unwrap();
        let book = novel.get_book_info(&document).unwrap();
        assert_eq!(
            book,
            Book {
                name: "射手兇猛".to_string(),
                author: "初四兮".to_string()
            }
        );
    }

    #[test]
    fn test_get_chapter_urls_sorted() {
        let document = visdom::Vis::load(CONTENTS).unwrap();
        let novel = UUkanshu::new("https://uukanshu.cc/book/20692/").unwrap();
        let urls = novel.get_chapter_urls_sorted(&document).unwrap();
        assert_eq!(
            urls.first().unwrap(),
            &Url::parse("https://uukanshu.cc/book/20692/11605757.html").unwrap()
        );
        assert_eq!(
            urls.last().unwrap(),
            &Url::parse("https://uukanshu.cc/book/20692/15472434.html").unwrap()
        );
    }

    #[test]
    fn test_get_chapter_content() {
        let document = visdom::Vis::load(CHAPTER).unwrap();
        let novel = UUkanshu::new("https://uukanshu.cc/book/20692/").unwrap();
        let chapter = novel.get_chapter(&document, "1").unwrap();
        assert_eq!(chapter.order, "1");
        assert_eq!(chapter.title, "第一章 老地方");
        assert!(!chapter.text.is_empty());
        let chapter = novel.process_chapter(chapter);
        assert!(chapter.text.starts_with("六月的首都日漸炎熱。"));
        assert!(chapter.text.ends_with("「開個機子。」"));
    }

    #[test]
    fn test_get_next_page() {
        let document = visdom::Vis::load(CHAPTER).unwrap();
        let novel = UUkanshu::new("https://uukanshu.cc/book/20692/").unwrap();
        assert_eq!(novel.get_next_page(&document).unwrap(), None);
    }
}
