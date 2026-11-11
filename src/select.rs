use crate::product::Product;
use crate::rng::Rng;

/// Pick `count` products at random, always listing on-sale products first.
///
/// The on-sale and regular groups are each shuffled, then concatenated
/// on-sale-first and truncated. On-sale products therefore always occupy the
/// leading positions; regular products only make up the balance when there are
/// fewer on-sale products than requested.
pub fn select_products(products: &[Product], count: usize, rng: &mut Rng) -> Vec<Product> {
    let take = count.min(products.len());
    if take == 0 {
        return Vec::new();
    }

    let mut on_sale: Vec<&Product> = products.iter().filter(|p| p.on_sale).collect();
    let mut regular: Vec<&Product> = products.iter().filter(|p| !p.on_sale).collect();

    shuffle(&mut on_sale, rng);
    shuffle(&mut regular, rng);

    on_sale
        .into_iter()
        .chain(regular)
        .take(take)
        .cloned()
        .collect()
}

/// Fisher-Yates shuffle.
pub fn shuffle<T>(items: &mut [T], rng: &mut Rng) {
    if items.len() < 2 {
        return;
    }
    for i in (1..items.len()).rev() {
        let j = rng.below((i + 1) as u64) as usize;
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product(id: u64, on_sale: bool) -> Product {
        Product {
            id,
            name: format!("Product {id}"),
            on_sale,
            is_in_stock: true,
            permalink: format!("https://example.test/product/{id}/"),
            thumbnail: None,
        }
    }

    fn make(count: u64, on_sale: bool, offset: u64) -> Vec<Product> {
        (0..count).map(|i| product(offset + i, on_sale)).collect()
    }

    #[test]
    fn returns_at_most_k_unique_products() {
        let products = make(20, false, 0);
        let picked = select_products(&products, 10, &mut Rng::from_seed(1));
        assert_eq!(picked.len(), 10);
        let unique: std::collections::HashSet<u64> = picked.iter().map(|p| p.id).collect();
        assert_eq!(unique.len(), 10);
    }

    #[test]
    fn caps_k_at_the_number_of_products() {
        let products = vec![product(1, false), product(2, false)];
        assert_eq!(select_products(&products, 10, &mut Rng::from_seed(1)).len(), 2);
    }

    #[test]
    fn zero_count_returns_nothing() {
        let products = make(5, true, 0);
        assert!(select_products(&products, 0, &mut Rng::from_seed(1)).is_empty());
    }

    #[test]
    fn always_lists_on_sale_products_first() {
        let mut products = make(6, false, 0);
        products.extend(make(4, true, 100));

        let picked = select_products(&products, 5, &mut Rng::from_seed(7));
        let flags: Vec<bool> = picked.iter().map(|p| p.on_sale).collect();
        assert_eq!(flags, vec![true, true, true, true, false]);
    }

    #[test]
    fn returns_only_on_sale_products_when_there_are_enough() {
        let mut products = make(15, true, 0);
        products.extend(make(30, false, 1000));

        let picked = select_products(&products, 10, &mut Rng::from_seed(99));
        assert_eq!(picked.len(), 10);
        assert!(picked.iter().all(|p| p.on_sale));
    }

    #[test]
    fn keeps_results_unique_when_mixing_groups() {
        let mut products = make(5, true, 0);
        products.extend(make(5, false, 100));

        let picked = select_products(&products, 8, &mut Rng::from_seed(3));
        let unique: std::collections::HashSet<u64> = picked.iter().map(|p| p.id).collect();
        assert_eq!(unique.len(), 8);
        assert_eq!(picked.iter().filter(|p| p.on_sale).count(), 5);
    }

    #[test]
    fn falls_back_to_regular_products_when_nothing_is_on_sale() {
        let products = make(10, false, 0);
        let picked = select_products(&products, 4, &mut Rng::from_seed(5));
        assert_eq!(picked.len(), 4);
        assert!(picked.iter().all(|p| !p.on_sale));
    }
}
