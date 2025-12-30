#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use serde_json::{json, Value};
    use md5::Md5;
    use md5::Digest;
    
    #[test]
    fn test_content_hash_matches_composer() {
        let mut relevant: BTreeMap<&str, Value> = BTreeMap::new();
        relevant.insert("name", json!("vendor/test"));
        
        let mut require: BTreeMap<&str, &str> = BTreeMap::new();
        require.insert("symfony/console", "*");
        relevant.insert("require", json!(require));
        
        let json_str = serde_json::to_string(&relevant).unwrap();
        let escaped = json_str.replace("/", "\\/");

        // Compute MD5
        let mut hasher = Md5::new();
        hasher.update(escaped.as_bytes());
        let result = hasher.finalize();
        let hash = format!("{:x}", result);

        // Expected from PHP: 952f760ba9cfb2ca4a799c52d42099d4
        assert_eq!(hash, "952f760ba9cfb2ca4a799c52d42099d4");
    }
}
