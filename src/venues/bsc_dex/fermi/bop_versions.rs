//! Идентичность рыночных ролей и исполнителей из записей Ethereum и подтверждённых CREATE.
use alloy::primitives::{B256, b256};
pub(super) const DIRECT: B256 =
    b256!("a2f5c022f7626f7bd9354973ec06bdad3e3aa96f6fa116040b791adeef90ddf4");
pub(super) const CASH: B256 =
    b256!("2a706b9d36defb89fc0b2b15ac6182c403ef6eefd8cf045c692624461fefca4b");
