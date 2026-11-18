//! Файл: tests/core_deposit_proof_411.rs
//! Что делает: решение 411 (задача t196, переделка «по триггеру»):
//! 1) чистая логика core/deposit_proof_plan.rs: compare — ответ триггерного запроса совпал с памятью / изменился / в
//!    памяти не было; use_memory — адрес из памяти без запроса, пока нет пометки «спросить заново»; тексты без адреса;
//! 2) память адресов gateway/addr_book.rs с НАСТОЯЩИМ хранилищем секретов (secrets/vault.rs): адрес, положенный под
//!    ключом API, после смены ключа в хранилище не находится сразу (до Core::secrets_changed) — тем же поиском, что у
//!    предпроверки и вывода; ответ запроса, во время которого ключ сменили, в память не кладётся; возврат прежнего
//!    ключа снова находит адрес; заблокированное хранилище — ничего;
//! 3) Memo::forget_all прогрева после смены секретов.
//! Адреса — настоящие адреса BSC из записи tests/data/native_2026-10-25 (вывод BNB с BingX 24.10: отправитель и
//! получатель транзакции); к биржам тест не обращается.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::deposit_proof_plan, arb::core::deposit_warm_plan, arb::gateway::addr_book,
//! arb::secrets, serde_json. Моков нет.

use arb::core::deposit_proof_plan::{Seen, changed_text, compare, doubt_text, use_memory};
use arb::core::deposit_warm_plan::Memo;
use arb::gateway::addr_book::{AddrBook, Mark};
use arb::secrets::Vault;
use arb::secrets::crypto::KdfParams;
use serde_json::Value;
use zeroize::Zeroizing;

const KDF: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};
/// Имя секрета ключа API (как в venues/kucoin/ctx.rs SECRET_API_KEY).
const KEY: &str = "kucoin.api_key";

/// Два настоящих адреса BSC из записи: (отправитель, получатель) транзакции вывода BNB с BingX.
fn two_addresses() -> (String, String) {
    let p = "tests/data/native_2026-10-25/arb_bingx_bnb_withdraw_2026-10-25.json";
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(p).expect("запись")).expect("JSON");
    let tx = &v["results"][0]["response"]["result"];
    let from = tx["from"].as_str().expect("from").to_string();
    let to = tx["to"].as_str().expect("to").to_string();
    assert_ne!(from, to);
    (from, to)
}

fn pw() -> Zeroizing<String> {
    Zeroizing::new("пароль теста 411".to_string())
}

/// Хранилище секретов во временной папке с ключом API `key`.
async fn vault_with(tag: &str, key: &str) -> Vault {
    let d = std::env::temp_dir().join(format!("arb-411-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let v = Vault::locked(d.join("secrets.enc"));
    v.create(pw(), KDF).await.unwrap();
    set_key(&v, key).await;
    v
}

async fn set_key(v: &Vault, key: &str) {
    v.change(pw(), KEY, Some(Zeroizing::new(key.to_string())), KDF)
        .await
        .unwrap();
}

#[test]
fn trigger_answer_same_changed_first() {
    let (a, b) = two_addresses();
    assert_eq!(compare(Some(&a), &a), Seen::Same, "совпал");
    assert_eq!(compare(Some(&a), &b), Seen::Changed, "изменился");
    assert_eq!(compare(None, &b), Seen::First, "в памяти не было");
}

#[test]
fn memory_unless_doubted() {
    assert!(
        use_memory(true, false),
        "в памяти, пометки нет — без запроса"
    );
    assert!(
        !use_memory(true, true),
        "пометка «спросить заново» — запрос"
    );
    assert!(!use_memory(false, false), "нет в памяти — запрос");
}

#[test]
fn texts_without_address() {
    let (a, b) = two_addresses();
    let t = changed_text("kucoin", "ESIM", "BEP20");
    assert!(
        t.contains("kucoin") && t.contains("ESIM") && t.contains("изменился"),
        "{t}"
    );
    let d = doubt_text(
        "bitget",
        "LAB",
        "BEP20",
        "перевод идёт 900 с при норме 600 с",
    );
    for s in [&t, &d] {
        assert!(
            !s.contains(&a) && !s.contains(&b) && !s.contains("0x"),
            "{s}"
        );
    }
}

/// Путь предпроверки и вывода ищет адрес по отпечатку, прочитанному в момент поиска (Mark::read): смена ключа в
/// хранилище — и адрес прежнего ключа уже не находится, без всякого secrets_changed.
#[tokio::test]
async fn key_change_hides_old_address_at_once() {
    let (a, _) = two_addresses();
    let v = vault_with("change", "ключ-аккаунта-1").await;
    let book: AddrBook<String> = AddrBook::default();
    let m1 = Mark::read(&v, KEY);
    book.keep(&m1, "ESIM".into(), "BEP20".into(), a.clone());
    assert_eq!(
        book.peek(&Mark::read(&v, KEY), "ESIM", "BEP20"),
        Some(a.clone())
    );
    set_key(&v, "ключ-аккаунта-2").await;
    assert_eq!(
        book.peek(&Mark::read(&v, KEY), "ESIM", "BEP20"),
        None,
        "новый ключ — адреса нет"
    );
    set_key(&v, "ключ-аккаунта-1").await;
    assert_eq!(
        book.peek(&Mark::read(&v, KEY), "ESIM", "BEP20"),
        Some(a),
        "тот же ключ снова — тот же аккаунт"
    );
    v.lock();
    assert_eq!(
        book.peek(&Mark::read(&v, KEY), "ESIM", "BEP20"),
        None,
        "секреты закрыты — ничего"
    );
}

/// Ключ сменили, пока шёл запрос адреса: ответ (адрес прежнего или неизвестного аккаунта) в память не кладётся.
#[tokio::test]
async fn answer_during_key_change_not_kept() {
    let (_, b) = two_addresses();
    let v = vault_with("during", "ключ-аккаунта-1").await;
    let book: AddrBook<String> = AddrBook::default();
    let before = Mark::read(&v, KEY);
    set_key(&v, "ключ-аккаунта-2").await;
    book.keep(&before, "LAB".into(), "BEP20".into(), b);
    assert_eq!(book.peek(&before, "LAB", "BEP20"), None);
    assert_eq!(book.peek(&Mark::read(&v, KEY), "LAB", "BEP20"), None);
}

#[test]
fn warm_memo_forget_all() {
    let k = ("ESIM".to_string(), "BEP20".to_string());
    let mut m = Memo::default();
    let _ = m.record(k.clone(), Ok(()));
    assert!(!m.todo(&k), "получен — не запрашивается");
    m.forget_failed();
    assert!(!m.todo(&k), "forget_failed полученное не трогает");
    m.forget_all();
    assert!(m.todo(&k), "после смены секретов — проверить заново");
    assert_eq!(m.counts(), (0, 0));
}
