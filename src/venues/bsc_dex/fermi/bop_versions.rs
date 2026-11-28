//! Идентичность рыночных ролей и исполнителей из записей Ethereum и подтверждённых CREATE.
use alloy::primitives::{Address, B256, address, b256};
pub(super) const DIRECT: B256 =
    b256!("a2f5c022f7626f7bd9354973ec06bdad3e3aa96f6fa116040b791adeef90ddf4");
pub(super) const CASH: B256 =
    b256!("2a706b9d36defb89fc0b2b15ac6182c403ef6eefd8cf045c692624461fefca4b");
pub(super) const ROLES: (B256, Address, B256, Address, B256) = (
    b256!("bae2a8ec2d449ec965078332fda882eec8d2097ea034a177cc2860a7b43e46d0"),
    address!("b09999a44e3240193193be9dd99cc73c8cc945a6"),
    b256!("bb16459ebfa29a02df6e294c624a56a5081e170a7f91c90650f04ca5b56b3dba"),
    address!("da7afeed021eafc1c1af9c362de477dad0396b81"),
    b256!("cc6b013888e9bc9bb487c5301fff645b5d91007cf88053d145963921f6e2d026"),
);
