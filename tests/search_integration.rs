//! Quick integration test for Tavily and Bocha search providers
//! Run: cargo test --test search_integration -- --nocapture

use riceprompt_engine::*;

#[tokio::test]
async fn test_tavily_web_search() {
    let provider = TavilyProvider::new("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");
    let options = SearchOptions {
        max_results: Some(3),
        ..Default::default()
    };

    let result = provider.search("Rust programming language", options).await;
    match result {
        Ok(resp) => {
            println!("=== Tavily Web Search ===");
            println!("Query: {}", resp.query);
            println!("Results: {}", resp.results.len());
            for r in &resp.results {
                println!("  - {} | {}", r.title, r.url);
            }
            assert!(!resp.results.is_empty(), "Should return results");
        }
        Err(e) => panic!("Tavily web search failed: {:?}", e),
    }
}

#[tokio::test]
async fn test_tavily_news_search() {
    let provider = TavilyProvider::new("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");
    let options = SearchOptions {
        max_results: Some(3),
        search_type: Some("news".to_string()),
        freshness: Some("week".to_string()),
        ..Default::default()
    };

    let result = provider.search("AI artificial intelligence", options).await;
    match result {
        Ok(resp) => {
            println!("=== Tavily News Search ===");
            println!("Query: {}", resp.query);
            println!("Results: {}", resp.results.len());
            for r in &resp.results {
                println!("  - {} | {}", r.title, r.url);
            }
            assert!(!resp.results.is_empty(), "Should return news results");
        }
        Err(e) => panic!("Tavily news search failed: {:?}", e),
    }
}

#[tokio::test]
async fn test_bocha_web_search() {
    let provider = BochaProvider::new("sk-38005c3e0f324d3b93ac76f8713f47d6");
    let options = SearchOptions {
        max_results: Some(3),
        ..Default::default()
    };

    let result = provider.search("Rust 编程语言", options).await;
    match result {
        Ok(resp) => {
            println!("=== Bocha Web Search ===");
            println!("Query: {}", resp.query);
            println!("Results: {}", resp.results.len());
            println!("Answer: {:?}", resp.answer);
            for r in &resp.results {
                println!("  - {} | {}", r.title, r.url);
            }
            assert!(!resp.results.is_empty(), "Should return results");
        }
        Err(e) => panic!("Bocha web search failed: {:?}", e),
    }
}

#[tokio::test]
async fn test_bocha_ai_search() {
    let provider = BochaProvider::new("sk-38005c3e0f324d3b93ac76f8713f47d6");
    let options = SearchOptions {
        max_results: Some(3),
        search_type: Some("ai".to_string()),
        ..Default::default()
    };

    let result = provider.search("什么是 Rust 编程语言", options).await;
    match result {
        Ok(resp) => {
            println!("=== Bocha AI Search ===");
            println!("Query: {}", resp.query);
            println!("Results: {}", resp.results.len());
            println!("Answer: {:?}", resp.answer);
            for r in &resp.results {
                println!("  - {} | {}", r.title, r.url);
            }
            assert!(!resp.results.is_empty(), "Should return results");
        }
        Err(e) => panic!("Bocha AI search failed: {:?}", e),
    }
}
