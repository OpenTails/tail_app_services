use std::collections::HashSet;

use serde::Deserialize;

/// The `tailco/v1/sale-items` response.
///
/// Only the product ids are needed; everything else (prices, variations,
/// currency) is ignored.
#[derive(Debug, Deserialize)]
struct SaleItemsResponse {
    items: Vec<SaleItem>,
}

#[derive(Debug, Deserialize)]
struct SaleItem {
    id: u64,
}

/// Parse the sale-items payload into the set of product ids that are on sale.
pub fn parse_sale_items(text: &str) -> Result<HashSet<u64>, serde_json::Error> {
    let response: SaleItemsResponse = serde_json::from_str(text)?;
    Ok(response.items.into_iter().map(|item| item.id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_the_item_ids_and_ignores_everything_else() {
        let json = r#"{
            "currency_code": "USD",
            "count": 2,
            "items": [
                { "id": 42, "type": "simple", "on_sale": true, "price": "899" },
                { "id": 7, "type": "variable", "variations": [{ "id": 8 }] }
            ]
        }"#;

        let ids = parse_sale_items(json).unwrap();
        assert_eq!(ids, HashSet::from([42, 7]));
    }

    #[test]
    fn accepts_an_empty_item_list() {
        assert!(parse_sale_items(r#"{"items":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn rejects_a_missing_or_invalid_item_list() {
        assert!(parse_sale_items(r#"{}"#).is_err());
        assert!(parse_sale_items(r#"{"items":[{"type":"simple"}]}"#).is_err());
        assert!(parse_sale_items(r#"[{"id":1}]"#).is_err());
    }
}
