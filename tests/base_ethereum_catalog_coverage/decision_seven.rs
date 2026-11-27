//! Сверка списка владельца по первичному каталогу двух сетей.
use super::{BTreeSet, Value};

/// Для решения 7 ключ строки включает сеть. Оборот берётся из суточного ответа DefiLlama.
#[test]
fn decision_seven_inventory_accounts_for_each_requested_network_and_exchange() {
    let answers = include_str!("../../docs/review/BASE_ETHEREUM_ANSWERS.md");
    let archive: Value = serde_json::from_str(include_str!(
        "../data/first_run_dex_2026-11-26/catalog_recheck.json"
    ))
    .unwrap();
    let section = answers.split_once("<a id=\"o61-inventory\"></a>").unwrap().1;
    let mut expected = BTreeSet::new();
    for (network, ids) in [
        ("Base", "5919 7984 8849 7945 8513 4725 5221 2803 8322 6675 2116 9146 7214 9153 534 4196 7626 7902 6049 8053 5181"),
        ("Ethereum", "7979 9436 2803 6650 2116 405 1447 7982 3104 8823 6049 7626 8767"),
    ] {
        expected.extend(ids.split_whitespace().map(|id| (network, id)));
    }
    let rows = section.lines().skip_while(|line| !line.starts_with("| Base |") && !line.starts_with("| Ethereum |"))
        .take_while(|line| line.starts_with("| "));
    for row in rows {
        let cells: Vec<_> = row.trim_matches('|').split('|').map(str::trim).collect();
        assert_eq!(cells.len(), 7, "{row}");
        let (network, id) = (cells[0], cells[1]);
        assert!(expected.remove(&(network, id)), "Лишняя или повторённая строка {network} {id}");
        let data = &archive[network.to_ascii_lowercase()]["volume"]["by_id"][id];
        assert_eq!(Some(cells[3].parse::<f64>().unwrap()), data.as_f64(), "{network} {id}");
        assert!(cells[2].contains("github.com/DefiLlama/dimension-adapters/blob/"), "{row}");
        assert!(cells[4..].iter().all(|cell| !cell.is_empty()), "{row}");
    }
    assert!(expected.is_empty(), "Пропущены строки {expected:?}");
}
