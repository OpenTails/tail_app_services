use std::collections::HashSet;

use serde::Serialize;
use serde_json::Value;

/// The projection of an upstream product that this proxy exposes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Product {
    pub id: u64,
    pub name: String,
    pub on_sale: bool,
    pub is_in_stock: bool,
    pub permalink: String,
    pub thumbnail: Option<String>,
    pub thumbhash: Option<String>,
}

/// Mark the products whose id appears in `sale_ids` as on sale.
///
/// The Store API's own `on_sale` flag is ignored; the `tailco/v1/sale-items`
/// endpoint is the source of truth for which products are discounted.
pub fn apply_sale_items(products: &mut [Product], sale_ids: &HashSet<u64>) {
    for product in products {
        product.on_sale = sale_ids.contains(&product.id);
    }
}

/// Parse the WooCommerce Store API response into `Product`s.
///
/// Products are extracted leniently: an entry that is missing an id or a name
/// is skipped rather than failing the whole batch, matching the upstream API's
/// occasional rough edges.
pub fn parse_products(text: &str) -> Result<Vec<Product>, serde_json::Error> {
    let raw: Vec<Value> = serde_json::from_str(text)?;
    Ok(raw.into_iter().filter_map(normalize).collect())
}

fn normalize(value: Value) -> Option<Product> {
    let id = value.get("id")?.as_u64()?;
    let name = value.get("name")?.as_str()?.to_string();

    let is_in_stock = value
        .get("is_in_stock")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let permalink = value
        .get("permalink")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let thumbnail = value
        .get("images")
        .and_then(Value::as_array)
        .and_then(|images| images.first())
        .and_then(|image| {
            image
                .get("thumbnail")
                .and_then(Value::as_str)
                .or_else(|| image.get("src").and_then(Value::as_str))
        })
        .map(str::to_string);

    Some(Product {
        id,
        name,
        // Filled in later from the sale-items endpoint.
        on_sale: false,
        is_in_stock,
        permalink,
        thumbnail,
        thumbhash: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_only_the_exposed_fields() {
        let json = r#"[{
            "id": 42,
            "name": "Tail",
            "on_sale": true,
            "is_in_stock": true,
            "permalink": "https://example.test/product/tail/",
            "slug": "tail",
            "prices": { "price": "100" },
            "images": [
                { "src": "big.jpg", "thumbnail": "thumb.jpg" }
            ]
        }]"#;

        let products = parse_products(json).unwrap();
        assert_eq!(
            products,
            vec![Product {
                id: 42,
                name: "Tail".to_string(),
                // The upstream `on_sale` flag is ignored; sale items are
                // applied separately.
                on_sale: false,
                is_in_stock: true,
                permalink: "https://example.test/product/tail/".to_string(),
                thumbnail: Some("thumb.jpg".to_string()),
                thumbhash: None,
            }]
        );
    }

    #[test]
    fn applies_sale_items_by_product_id() {
        let mut products =
            parse_products(r#"[{"id":1,"name":"A"},{"id":2,"name":"B"},{"id":3,"name":"C"}]"#)
                .unwrap();

        apply_sale_items(&mut products, &HashSet::from([1, 3]));

        let flags: Vec<bool> = products.iter().map(|p| p.on_sale).collect();
        assert_eq!(flags, vec![true, false, true]);
    }

    #[test]
    fn falls_back_to_the_full_image_and_tolerates_missing_data() {
        let products = parse_products(
            r#"[{"id":1,"name":"A","images":[{"src":"big.jpg"}]},{"id":2,"name":"B"},{"name":"no id"}]"#,
        )
        .unwrap();

        assert_eq!(products.len(), 2, "entry without an id is skipped");
        assert_eq!(products[0].thumbnail.as_deref(), Some("big.jpg"));
        assert_eq!(products[1].thumbnail, None);
        assert!(!products[1].on_sale);
        assert!(!products[1].is_in_stock);
    }

    #[test]
    fn rejects_a_non_array_payload() {
        assert!(parse_products(r#"{"oops":true}"#).is_err());
    }
}
