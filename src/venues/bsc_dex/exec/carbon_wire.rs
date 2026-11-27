//! Подписанные действия контроллера Carbon. Предел, режим и физическая валюта сохраняются вместе с идентификаторами стратегий.
use super::super::{
    math::carbon_match,
    pools::{
        carbon::{NATIVE, TradeAction},
        carbon_quote,
    },
};
use crate::core::types::CarbonState;
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};

sol! {
    interface IWallet {
        function controller() external view returns(address);
        function execute(address source,address target,TradeAction[] actions,bool byTarget,uint256 deadline,uint128 limit) external payable returns(uint256 spent,uint256 received);
        event Settled(address indexed payer,address indexed source,address indexed target,uint256 spent,uint256 received);
    }
    interface ITrade {
        function tradeBySourceAmount(address sourceToken,address targetToken,TradeAction[] tradeActions,uint256 deadline,uint128 minReturn) external payable returns(uint128);
        function tradeByTargetAmount(address sourceToken,address targetToken,TradeAction[] tradeActions,uint256 deadline,uint128 maxInput) external payable returns(uint128);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Action {
    pub id: U256,
    pub amount: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Call {
    #[serde(default)]
    pub code: Option<super::carbon_binding::Code>,
    #[serde(default)]
    pub module: Option<Address>,
    pub controller: Address,
    pub input: Address,
    pub output: Address,
    pub source: Address,
    pub target: Address,
    pub by_target: bool,
    pub actions: Vec<Action>,
    pub limit: u128,
    pub deadline: U256,
}

impl Call {
    pub fn new(
        s: &CarbonState,
        input: Address,
        p: &carbon_match::Plan,
        limit: u128,
        deadline: U256,
    ) -> Option<Self> {
        let (source, target) = carbon_quote::contracts(s, input)?;
        let output = *s.tokens.iter().find(|a| **a != input)?;
        let c = Self {
            code: None,
            module: None,
            controller: s.controller,
            input,
            output,
            source,
            target,
            by_target: p.by_target,
            actions: p
                .actions
                .iter()
                .map(|a| Action {
                    id: a.id,
                    amount: a.amount,
                })
                .collect(),
            limit,
            deadline,
        };
        (c.total()? == if p.by_target { p.received } else { p.spent }
            && p.spent > 0
            && p.received > 0
            && if p.by_target {
                p.spent <= limit
            } else {
                p.received >= limit
            })
        .then_some(c)
    }
    pub fn total(&self) -> Option<u128> {
        if self.actions.is_empty()
            || self.deadline.is_zero()
            || self.limit == 0
            || self.source == self.target
            || self.input == self.output
            || self.controller.is_zero()
        {
            return None;
        }
        if (self.source != NATIVE && self.source != self.input)
            || (self.target != NATIVE && self.target != self.output)
        {
            return None;
        }
        let mut seen = std::collections::HashSet::new();
        self.actions.iter().try_fold(0u128, |n, a| {
            (a.amount > 0 && !a.id.is_zero() && seen.insert(a.id)).then_some(())?;
            n.checked_add(a.amount)
        })
    }
    pub fn recipient(&self) -> Address {
        self.module.unwrap_or(self.controller)
    }
    pub fn wrapped(&self) -> Option<Address> {
        if self.source == NATIVE {
            Some(self.input)
        } else if self.target == NATIVE {
            Some(self.output)
        } else {
            None
        }
    }
    pub fn cap(&self) -> U256 {
        if self.by_target {
            U256::from(self.limit)
        } else {
            U256::from(self.total().unwrap_or_default())
        }
    }
    pub fn value(&self) -> U256 {
        if self.source == NATIVE {
            self.cap()
        } else {
            U256::ZERO
        }
    }
    pub fn data(&self) -> Bytes {
        let actions = self
            .actions
            .iter()
            .map(|a| TradeAction {
                strategyId: a.id,
                amount: a.amount,
            })
            .collect();
        if self.module.is_some() {
            return IWallet::executeCall {
                source: self.source,
                target: self.target,
                actions,
                byTarget: self.by_target,
                deadline: self.deadline,
                limit: self.limit,
            }
            .abi_encode()
            .into();
        }
        if self.by_target {
            ITrade::tradeByTargetAmountCall {
                sourceToken: self.source,
                targetToken: self.target,
                tradeActions: actions,
                deadline: self.deadline,
                maxInput: self.limit,
            }
            .abi_encode()
            .into()
        } else {
            ITrade::tradeBySourceAmountCall {
                sourceToken: self.source,
                targetToken: self.target,
                tradeActions: actions,
                deadline: self.deadline,
                minReturn: self.limit,
            }
            .abi_encode()
            .into()
        }
    }
    pub fn tighten(&self, spent: U256, received: U256) -> Option<Self> {
        let limit = if self.by_target {
            u128::try_from(spent).ok()?.checked_sub(1)?
        } else {
            u128::try_from(received).ok()?.checked_add(1)?
        };
        Some(Self {
            limit,
            ..self.clone()
        })
    }
}
