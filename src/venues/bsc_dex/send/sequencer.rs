//! Файл: venues/bsc_dex/send/sequencer.rs
//! Что делает: решение 435 (задача 433-е) — канал «секвенсор» сети Arbitrum One: одиночная отправка подписанной
//! транзакции eth_sendRawTransaction на адрес секвенсора (channel.url — настройка, не код). Мемпула, бандлов и
//! билдеров у сети нет (docs/notes/arbitrum/docs-check-2026-11-20.md) — канал только одиночный, чаевых билдеру нет
//! (ставка очереди — чаевые самой транзакции типа 2, send.priority_fee_gwei). Канал сети Arbitrum: channel.chain =
//! id сети площадки arb_dex (send/channels.rs serves) — площадка BSC его не берёт.
//! eth_sendRawTransactionConditional (условия knownAccounts, окна блока L1 и времени L2, ошибка −32003) в документации
//! описан без формата запроса и примера — не реализован, вопрос Тиграну (docs/TASKS.md, «Решение 435»).
//! Открытый ли это канал в смысле решения 33, документация прямо не говорит: эндпоинт публичный, приватности не
//! обещает — канал помечен открытым (только один, не вместе с закрытыми; на площадке Arbitrum других каналов нет).
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [sequencer]: channel.url (официально https://arb1-sequencer.arbitrum.io/rpc), channel.method
//! = single, channel.chain, channel.min_gas_price_gwei, channel.enabled.
//!
//! Документация: https://docs.arbitrum.io/node-running/node-providers (раздел о конечной точке секвенсора: «accepts
//! only eth_sendRawTransaction and eth_sendRawTransactionConditional», очередь по умолчанию 1024 транзакции, срок
//! 12 с, «context deadline exceeded» — транзакция не принята, повтор безопасен; успешный ответ — мягкое подтверждение);
//! https://docs.arbitrum.io/arbitrum-essentials/arbitrum-vs-ethereum/rpc-methods (eth_sendRawTransactionConditional);
//! метод eth_sendRawTransaction — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/submit.yaml .

use super::channel::{Auth, Channel, Single};

/// Канал секвенсора Arbitrum One.
pub const CHANNEL: Channel = Channel {
    id: "sequencer",
    title: "Секвенсор Arbitrum (одиночная отправка)",
    private: false,
    single: Some(Single::Raw),
    bundle: None,
    tips: false,
    auth: Auth::None,
    docs: &[
        "https://docs.arbitrum.io/node-running/node-providers",
        "https://docs.arbitrum.io/arbitrum-essentials/arbitrum-vs-ethereum/rpc-methods",
        "https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/submit.yaml",
    ],
};
