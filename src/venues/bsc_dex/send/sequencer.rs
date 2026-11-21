//! Файл: venues/bsc_dex/send/sequencer.rs
//! Что делает: решение 435 (задача 433-е) — канал «секвенсор» сети Arbitrum One: одиночная отправка подписанной
//! транзакции eth_sendRawTransaction на адрес секвенсора (channel.url — настройка, не код). Мемпула, бандлов и
//! билдеров у сети нет (docs/notes/arbitrum/docs-check-2026-11-20.md) — канал только одиночный, чаевых билдеру нет
//! (ставка очереди — чаевые самой транзакции типа 2, send.priority_fee_gwei). Канал сети Arbitrum: channel.chain =
//! id сети площадки arb_dex (send/channels.rs serves) — площадка BSC его не берёт.
//! Решение 436 (А): при окне channel.cond_time_window_s — eth_sendRawTransactionConditional с timestampMax (формат —
//! по исходникам Nitro, send/sequencer_cond.rs); окно не задано — обычная eth_sendRawTransaction. Решение 436 (Б):
//! минимум цены газа канала — с узла (ArbGasInfo.getMinimumGasPrice, node/arb_min_gas.rs, dex.arb_gas_info), а не
//! channel.min_gas_price_gwei. Вид отправки — Single::Sequencer (send/channel.rs).
//! Открытый ли это канал в смысле решения 33, документация прямо не говорит: эндпоинт публичный, приватности не
//! обещает — канал помечен открытым (только один, не вместе с закрытыми; на площадке Arbitrum других каналов нет).
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [sequencer]: channel.url (официально https://arb1-sequencer.arbitrum.io/rpc), channel.method
//! = single, channel.chain, channel.cond_time_window_s (решение 436), channel.enabled; минимум газа — dex.arb_gas_info
//! [площадка DEX] (channel.min_gas_price_gwei у этого канала не читается, решение 436).
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
    single: Some(Single::Sequencer),
    bundle: None,
    tips: false,
    auth: Auth::None,
    docs: &[
        "https://docs.arbitrum.io/node-running/node-providers",
        "https://docs.arbitrum.io/arbitrum-essentials/arbitrum-vs-ethereum/rpc-methods",
        "https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/submit.yaml",
        "https://github.com/OffchainLabs/go-ethereum/blob/260bc5dcbd5a16925d7e26145577b2612f30f428/arbitrum_types/txoptions.go",
        "https://docs.arbitrum.io/notices/eth-send-raw-transaction-conditional-deprecation-notice",
    ],
};
