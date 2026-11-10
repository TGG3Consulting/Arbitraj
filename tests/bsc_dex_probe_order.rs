//! Файл: tests/bsc_dex_probe_order.rs
//! Что делает: порядок вызовов второй симуляции пробы налогов (решение 252, check/probe_calls.rs sell_tail) — продажа
//! монеты идёт ДО пробного перевода монеты на пустой адрес, как у настоящей продажи (перед ней перевода монеты в том же
//! блоке нет). Живые случаи PIG и SMARS 09–10.11 (docs/notes/reflection-tax-2026-11-10.md): монеты с отражением
//! (reflection), перевод перед продажей поднимает balanceOf пары V2 выше резерва — проба занижала налог продажи.
//! Адреса — из фактов сервера (docs/notes/tax-trace-2026-11-10.md): монета PIG, WBNB, кошелёк w2. Данные вызовов
//! квотера и продажи в этом тесте не важны (проверяется только порядок и адресаты) — пустые.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::check::{probe_calls, simulate}, node/abi.rs (IERC20).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256, address, keccak256};

use arb::venues::bsc_dex::check::probe_calls::{self as pc, Tail};
use arb::venues::bsc_dex::check::simulate::Call;
use arb::venues::bsc_dex::node::abi::IERC20;

const PIG: Address = address!("8850d2c68c632e3b258e612abaa8fada7e6958e5");
const WBNB: Address = address!("bb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");
const W2: Address = address!("a9C3CACff416e644Dec51986DFD0E17Cc2132817");
/// Пара PIG/WBNB (tax-trace) — здесь только метка адресата вызовов квотера и продажи (проверяется порядок).
const ROUTER: Address = address!("51dcaf423fe39f620a13379cd26821cf8d433308");

#[test]
fn sale_goes_before_probe_transfer() {
    let empty = Address::from_slice(&keccak256(W2.as_slice())[12..]);
    let quarter = U256::from(1_766_305_285_479_558_519u64);
    let approvals = vec![(PIG, Bytes::new())];
    // Начало списка — вызовы покупки (их число не важно для хвоста).
    let mut list: Vec<Call> = vec![pc::balance(W2, PIG, W2); 3];
    let t: Tail = pc::sell_tail(
        &mut list,
        (W2, PIG, WBNB, empty),
        quarter,
        &approvals,
        (ROUTER, Bytes::new()),
        (ROUTER, Bytes::new()),
    );
    // Разрешение продажи — сразу за покупкой, затем квотер, баланс, продажа, баланс, перевод, баланс пустого адреса.
    assert_eq!(
        t,
        Tail {
            quote: 4,
            transfer: 8
        }
    );
    assert_eq!(list.len(), 10);
    assert_eq!(list[3].to, PIG);
    let sale = t.quote + 2;
    assert_eq!(list[sale].to, ROUTER);
    assert!(sale < t.transfer, "продажа — до пробного перевода монеты");
    // Ни одного перевода монеты между покупкой и продажей.
    let transfer_sel = IERC20::transferCall::SELECTOR;
    assert!(
        list[3..=sale]
            .iter()
            .all(|c| !(c.to == PIG && c.input.starts_with(&transfer_sel)))
    );
    let tr = &list[t.transfer];
    assert_eq!((tr.from, tr.to), (W2, PIG));
    assert_eq!(
        tr.input,
        Bytes::from(
            IERC20::transferCall {
                to: empty,
                value: quarter
            }
            .abi_encode()
        )
    );
    // Баланс встречного токена — до и после продажи; баланс монеты пустого адреса — после перевода.
    let bal = |owner| Bytes::from(IERC20::balanceOfCall { owner }.abi_encode());
    assert_eq!((list[sale - 1].to, &list[sale - 1].input), (WBNB, &bal(W2)));
    assert_eq!((list[sale + 1].to, &list[sale + 1].input), (WBNB, &bal(W2)));
    assert_eq!(
        (list[t.transfer + 1].to, &list[t.transfer + 1].input),
        (PIG, &bal(empty))
    );
}
