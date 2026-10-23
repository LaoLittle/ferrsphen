use pinyin::ToPinyin;

#[derive(Default)]
pub struct SearchIndex {
    entries: std::collections::HashMap<String, (String, String)>,
}
impl SearchIndex {
    pub fn matches(&mut self, title: &str, query: &str) -> bool {
        let query = compact(query);
        if query.is_empty() || compact(title).contains(&query) {
            return true;
        }
        let (full, initials) = self.entries.entry(title.to_owned()).or_insert_with(|| {
            let mut full = String::new();
            let mut initials = String::new();
            for character in title.chars() {
                if let Some(pinyin) = character.to_pinyin() {
                    full.push_str(pinyin.plain());
                    initials.push(pinyin.plain().chars().next().unwrap());
                } else if !character.is_whitespace() {
                    full.extend(character.to_lowercase());
                    initials.extend(character.to_lowercase());
                }
            }
            (full, initials)
        });
        full.contains(&query) || initials.contains(&query)
    }
}
fn compact(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_matches_chinese_full_pinyin_initials_and_mixed_names() {
        let mut index = SearchIndex::default();
        for query in ["", "测试", "ceshi", "CSQ", "ce shi qun", "试群"] {
            assert!(index.matches("测试群", query), "{query}");
        }
        for query in ["Rustkaifa", "rustkf", "开发", "RUST"] {
            assert!(index.matches("Rust开发", query), "{query}");
        }
        assert!(!index.matches("测试群", "ceshikaifa"));
    }
}
