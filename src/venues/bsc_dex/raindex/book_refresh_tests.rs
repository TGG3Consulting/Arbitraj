use super::*;
use serde_json::Value;

#[test]
fn slow_book_does_not_hide_or_extend_another_books_recorded_payment() {
    let bytes = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_deployment_balances.json.gz"
    );
    let doc: Value =
        serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
    let real = doc["sample_replay"]["protected"]["cases"]
        .as_array()
        .unwrap();
    let offer = |v| {
        let r = real
            .iter()
            .find(|r| r["stage"] == "paid" && r["version"].as_u64() == Some(v))
            .unwrap();
        (
            r["book"].as_str().unwrap().parse::<Address>().unwrap(),
            r["order"].as_str().unwrap().parse::<B256>().unwrap(),
        )
    };
    let (first, a) = offer(3);
    let (second, b) = offer(6);
    let ttl = Duration::from_secs(2);
    let t = Instant::now();
    let later = t + ttl / 2;
    let mut samples = Books(BTreeMap::new());
    samples.replace(first, t, vec![a]);
    samples.replace(second, later, vec![b]);
    assert_eq!(samples.oldest(), Some(t));
    samples.expire(t + ttl, ttl);
    assert_eq!(samples.0.len(), 1);
    assert_eq!(samples.0[&second].1, [b]);
    assert_eq!(samples.oldest(), Some(later));
    samples.replace(first, t, vec![]);
    assert_eq!(samples.oldest(), Some(later));
    samples.expire(later + ttl, ttl);
    assert!(samples.0.is_empty());
}
