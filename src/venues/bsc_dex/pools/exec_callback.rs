//! Файл: venues/bsc_dex/pools/exec_callback.rs
//! Что делает: ревью 281, В4 — обратный вызов обмена пула V3-форка, которого может не быть у контракта-исполнителя.
//! Пул SquadSwap V3 в swap зовёт у вызывающего `squadV3SwapCallback(int256,int256,bytes)` (0xc4e21d86, SquadV3Pool.sol
//! L794, L800), а не uniswapV3SwapCallback / pancakeV3SwapCallback, которые есть у ArbExecutor v1 — шаг V3 с таким пулом
//! откатится. Необязательная настройка DEX dex.v3_swap_callback — сигнатура обратного вызова, который зовёт пул (Squad:
//! `squadV3SwapCallback(int256,int256,bytes)`). Задана — её селектор (первые 4 байта keccak256 сигнатуры) ищется в коде
//! исполнителя contracts/out/ArbExecutor.bin-runtime (тот же артефакт, хеш которого сверен и с которым бот сверяет код
//! развёрнутого исполнителя — exec/executor/{code, onchain}.rs) как PUSH4 селектора (0x63 + 4 байта — так диспетчер
//! solc сравнивает селектор вызова; у pancakeV3SwapCallback 0x23a69e75 / uniswapV3SwapCallback 0xfa461e33 в артефакте
//! есть, у 0xc4e21d86 — нет, 15.11.2026). Нет — пулы DEX без пути исполнения через исполнитель (exec_kind.rs
//! via_executor = нет): в маршруты и бандлы не идут, проба налогов через исполнитель для них не запускается (NoRoute:
//! «не проверено», монета не голосуется — как у вида без шага исполнителя, check/probe_exec.rs). Когда вызов добавят в
//! исполнитель и пересоберут артефакт — заработает без правок бота. Не задана — проверки нет (как раньше).
//! Кто вызывает: pools/exec_kind.rs (via_executor, describe), check/probe_exec.rs (no_path).
//! От чего зависит: ctx.rs, families.rs, registry_key.rs (selector), exec/executor/code.rs (runtime).
//! Настройки: dex.v3_swap_callback [DEX] (необязательная).

use std::collections::HashMap;
use std::sync::Mutex;

use super::super::ctx::Ctx;
use super::super::exec::executor::code;
use super::families::Family;
use super::registry_key;

/// Имя настройки.
pub const SETTING: &str = "dex.v3_swap_callback";

/// Итог поиска по сигнатуре (артефакт неизменен до перезапуска): None — есть, Some — причина.
static SEEN: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

/// Есть ли PUSH4 селектора в коде (0x63 и 4 байта селектора подряд). Принимает: код, селектор. Ошибок нет.
pub fn has_selector(runtime: &[u8], sel: &[u8]) -> bool {
    let mut want = vec![0x63u8];
    want.extend_from_slice(sel);
    runtime.windows(want.len()).any(|w| w == want.as_slice())
}

/// Причина «исполнитель не поддерживает обратный вызов» по сигнатуре (без памяти). Ошибок нет: артефакт не сверен —
/// тоже причина (не исполняем вслепую).
fn check(sig: &str) -> Option<String> {
    let sel = registry_key::selector(sig);
    match code::runtime() {
        Ok(rt) if has_selector(&rt, &sel) => None,
        Ok(_) => Some(format!(
            "исполнитель не поддерживает обратный вызов {sig} (селектор {sel}) — нужен исполнитель новой версии"
        )),
        Err(why) => Some(format!("код исполнителя не сверен ({why}) — обратный вызов {sig} не проверить")),
    }
}

/// Нет ли у исполнителя обратного вызова, который зовёт пул этой DEX. Принимает: окружение, вид. Возвращает: None —
/// настройка не задана или вызов есть; Some — причина. Ошибок нет.
pub fn missing(ctx: &Ctx, f: &Family) -> Option<String> {
    let sig = ctx.dex_text(SETTING, f.id).ok()?.trim().to_string();
    if sig.is_empty() {
        return None;
    }
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new)
        .entry(sig.clone())
        .or_insert_with(|| check(&sig))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Селектор squadV3SwapCallback = 0xc4e21d86 (SquadV3Pool.sol L794); в артефакте v1 его нет, а
    /// pancakeV3SwapCallback / uniswapV3SwapCallback — есть (тот же поиск PUSH4).
    #[test]
    fn squad_callback_absent_in_executor_v1() {
        let squad = registry_key::selector("squadV3SwapCallback(int256,int256,bytes)");
        assert_eq!(squad.to_vec(), vec![0xc4, 0xe2, 0x1d, 0x86]);
        let rt = code::runtime().unwrap();
        assert!(!has_selector(&rt, &squad));
        for sig in [
            "pancakeV3SwapCallback(int256,int256,bytes)",
            "uniswapV3SwapCallback(int256,int256,bytes)",
        ] {
            assert!(has_selector(&rt, &registry_key::selector(sig)), "{sig}");
        }
        assert!(check("squadV3SwapCallback(int256,int256,bytes)").is_some());
        assert!(check("pancakeV3SwapCallback(int256,int256,bytes)").is_none());
    }
}
