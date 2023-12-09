/// 稷下書院 <https://www.novel543.com/>
use super::{Book, Chapter, NovelError, Noveler};
use async_trait::async_trait;
use std::fmt::{self, Display};
use url::Url;
use visdom::types::Elements;

pub(crate) struct Novel543 {
    base: Url,
}

impl Novel543 {
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

        Ok(Self { base })
    }
}

impl Display for Novel543 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "稷下書院")
    }
}

#[async_trait]
impl Noveler for Novel543 {
    fn get_book_info(&self, document: &Elements) -> Result<Book, NovelError> {
        let selector = r"h1.title.is-2";
        let name = document.find(selector).text().replace(" 章節列表", "");

        let selector = r"h2.title.is-4";
        let author = document.find(selector).text().replace("作者 / ", "");
        Ok(Book { name, author })
    }

    fn get_chapter_urls_sorted(&self, document: &Elements) -> Result<Vec<Url>, NovelError> {
        let selector = r"ul.flex.one.two-700.three-900.all > li > a";
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
        let selector = r"#chapterWarp > div.chapter-content.px-3 > h1";
        let title = document.find(selector).text().trim().to_string();

        let selector = r"#chapterWarp > div.chapter-content.px-3 > div";
        let text: String = document.find(selector).text();

        let order = order.to_string();
        Ok(Chapter { order, title, text })
    }

    fn get_next_page(&self, document: &Elements) -> Result<Option<Url>, NovelError> {
        let selector = r"head > link:nth-last-of-type(1)";
        let curr_page = document
            .find(selector)
            .attr("href")
            .ok_or(NovelError::NotFound("curr_page href".to_string()))?
            .to_string();
        let curr_page = Url::parse(&curr_page)?;

        // std::fs::write("test.html", &document.html())?;
        let selector = r"#read > div > div.warp.my-5.foot-nav > a:nth-child(5)";
        let next_page = document
            .find(selector)
            .attr("href")
            .ok_or(NovelError::NotFound("next_page href".to_string()))?
            .to_string();

        let relative = self
            .base
            .make_relative(&curr_page)
            .ok_or(NovelError::NotFound("curr_page relative".to_string()))?;

        if next_page.contains(&relative.replace(".html", "")) {
            Ok(Some(self.base.join(&next_page)?))
        } else {
            Ok(None)
        }
    }

    fn process_chapter(&self, chapter: Chapter) -> Chapter {
        let mut text = chapter.text.trim().to_string();
        text = text
            .split_inclusive('。')
            .map(|s| s.trim().replace('㱕', ""))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Chapter { text, ..chapter }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static CONTENTS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/novel543/contents.html"
    ));
    static CHAPTER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/novel543/chapter.html"
    ));

    #[test]
    fn test_get_book_info() {
        let html = CONTENTS;
        let document = visdom::Vis::load(html).unwrap();
        let novel = Novel543::new("https://www.novel543.com/0413188175/dir").unwrap();
        let book = novel.get_book_info(&document).unwrap();
        assert_eq!(
            book,
            Book {
                name: "我的大寶劍".to_string(),
                author: "學霸殿下".to_string()
            }
        );
    }

    #[test]
    fn test_get_chapter_urls_sorted() {
        let html = CONTENTS;
        let document = visdom::Vis::load(html).unwrap();
        let novel = Novel543::new("https://www.novel543.com/0413188175/dir").unwrap();
        let urls = novel.get_chapter_urls_sorted(&document).unwrap();
        assert_eq!(
            urls.first().unwrap(),
            &Url::parse("https://www.novel543.com/0413188175/8001_1.html").unwrap()
        );
        assert_eq!(
            urls.last().unwrap(),
            &Url::parse("https://www.novel543.com/0413188175/8001_1250.html").unwrap()
        );
    }

    #[test]
    fn test_get_chapter_content() {
        let html = CHAPTER;
        let document = visdom::Vis::load(html).unwrap();
        let novel = Novel543::new("https://www.novel543.com/0413188175/dir").unwrap();
        let chapter = novel.get_chapter(&document, "1").unwrap();
        assert_eq!(chapter.order, "1".to_string());
        assert_eq!(
            chapter.title,
            "我的大寶劍 - 第一章 這不是性騷擾,所以不許投訴我! (1/2)".to_string()
        );
        let chapter = novel.process_chapter(chapter);
        dbg!(&chapter.text);
        assert!(chapter.text.starts_with("時為始皇曆1840年"));
        assert!(chapter.text.ends_with("可是相當相當寶貴人生經驗啊。"));
    }

    #[test]
    fn test_get_next_page() {
        let html = CHAPTER;
        let document = visdom::Vis::load(html).unwrap();
        let novel = Novel543::new("https://www.novel543.com/0413188175/dir").unwrap();
        let url = novel.get_next_page(&document).unwrap().unwrap();
        assert_eq!(
            url,
            Url::parse("https://www.novel543.com/0413188175/8001_316_2.html").unwrap(),
        );
    }
}
