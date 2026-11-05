/// Every URL the hover cards can produce must be a page the website publishes.
#[test]
fn every_anchor_url_is_a_real_page() {
    let d = saule_docs::stdlib();
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../www/dist");
    if !dist.exists() {
        eprintln!("www/dist not built — skipping");
        return;
    }
    let mut checked = 0;
    let mut bad = Vec::new();
    let mut urls: Vec<String> = d.all().iter().map(|e| e.url.clone()).collect();
    for name in [
        "String", "Math", "Os", "Io", "File", "Table", "Iter", "Bytes", "Saule", "Project",
    ] {
        if let Some(m) = d.module(name) {
            urls.push(m.url.clone());
        }
    }
    for url in urls {
        let path = url
            .trim_start_matches("https://lauriszz123.github.io/saule/")
            .split('#')
            .next()
            .unwrap()
            .trim_end_matches('/')
            .to_string();
        let file = dist.join(&path).join("index.html");
        checked += 1;
        if !file.exists() {
            bad.push(format!("{url} -> {}", file.display()));
        }
    }
    assert!(bad.is_empty(), "{checked} checked, dead: {bad:#?}");
    println!("{checked} documentation URLs all resolve to built pages");
}
