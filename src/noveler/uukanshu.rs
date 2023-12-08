/// UU看書 <https://www.uukanshu.com/>
use super::{Book, Chapter, NovelError, Noveler};
use async_trait::async_trait;
use regex::Regex;
use url::Url;
use visdom::types::Elements;

pub(crate) struct UUkanshu {
    base: Url,
    replacer: (Vec<Regex>, Vec<String>),
}

impl UUkanshu {
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

        let patterns = ["(?s)如果喜歡.*，請把網址發給您的朋友。.*"];
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

#[async_trait]
impl Noveler for UUkanshu {
    fn get_book_info(&self, document: &Elements) -> Result<Book, NovelError> {
        let selector = r"dd.jieshao_content > h1 > a";
        let name = document.find(selector).text().replace("最新章節", "");

        let selector = r"dd.jieshao_content > h2 > a";
        let author = document.find(selector).text();
        Ok(Book { name, author })
    }

    fn get_chapter_urls_sorted(&self, document: &Elements) -> Result<Vec<Url>, NovelError> {
        let selector = r"ul#chapterList a";
        let urls = document
            .find(selector)
            .into_iter()
            .map(|x| {
                x.get_attribute("href")
                    .map(|attr| attr.to_string())
                    .ok_or(NovelError::NotFound("href".to_string()))
            })
            .map(|x| x.and_then(|url_str| self.base.join(&url_str).map_err(NovelError::ParseError)))
            .collect::<Result<Vec<Url>, NovelError>>()?;
        Ok(urls.into_iter().rev().collect())
    }

    fn get_chapter(&self, document: &Elements, order: &str) -> Result<Chapter, NovelError> {
        let selector = r"h1#timu";
        let title = document.find(selector).text().trim().to_string();

        let selector = r"div#contentbox.uu_cont";
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
            .split(['\n', '\u{3000}'])
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
        let html = CONTENTS;
        let document = visdom::Vis::load(html).unwrap();
        let novel = UUkanshu::new("https://tw.uukanshu.com/b/239329/").unwrap();
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
        let html = CONTENTS;
        let document = visdom::Vis::load(html).unwrap();
        let novel = UUkanshu::new("https://tw.uukanshu.com/b/239329/").unwrap();
        let urls = novel.get_chapter_urls_sorted(&document).unwrap();
        assert_eq!(
            urls.first().unwrap(),
            &Url::parse("https://tw.uukanshu.com/b/239329/176659.html").unwrap()
        );
        assert_eq!(
            urls.last().unwrap(),
            &Url::parse("https://tw.uukanshu.com/b/239329/374018.html").unwrap()
        );
    }

    #[test]
    fn test_get_chapter_content() {
        let html = CHAPTER;
        let document = visdom::Vis::load(html).unwrap();
        let novel = UUkanshu::new("https://tw.uukanshu.com/b/239329/").unwrap();
        let chapter = novel.get_chapter(&document, "1").unwrap();
        assert_eq!(chapter.order, "1".to_string());
        assert_eq!(chapter.title, "第1章 老地方".to_string());
        assert!(!chapter.text.is_empty());
        let chapter = novel.process_chapter(chapter);
        dbg!(&chapter.text);
        assert!(chapter.text.starts_with("六月的首都日漸炎熱。"));
        assert!(chapter.text.ends_with("“開個機子。”"));
    }

    #[test]
    fn test_get_next_page() {
        let html = CHAPTER;
        let document = visdom::Vis::load(html).unwrap();
        let novel = UUkanshu::new("https://tw.uukanshu.com/b/239329/").unwrap();
        let url = novel.get_next_page(&document).unwrap();
        assert_eq!(url, None);
    }
}
