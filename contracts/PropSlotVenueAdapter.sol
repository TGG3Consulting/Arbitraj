// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

import {IPublicPropRouter, PropVenueAdapter} from "./PropVenueAdapter.sol";

/// @notice One named market on one Ethereum parent and slot, using the same custody checks as a normal swap.
contract PropSlotVenueAdapter is PropVenueAdapter {
    struct SlotOrder {
        bytes32 leg;
        bytes32 parent;
        uint64 height;
        uint64 timestamp;
        address tokenIn;
        address tokenOut;
        uint256 amount;
        uint256 minimum;
    }

    error WrongSlot();
    event SlotFilled(bytes32 indexed leg, address indexed payer, address indexed venue, uint256 spent, uint256 received);

    constructor(IPublicPropRouter publicRouter, address selected, address cashVault, bytes32 shell, bytes32 venueCode, bytes32 cashCode)
        PropVenueAdapter(publicRouter, selected, cashVault, shell, venueCode, cashCode) {}

    function slotVersion() external pure returns (uint256) { return 1; }

    function executeAtSlot(SlotOrder calldata order) external returns (uint256 spent, uint256 received) {
        if (order.leg==bytes32(0) || order.parent==bytes32(0) || block.chainid!=1 ||
            block.number!=order.height || block.timestamp!=order.timestamp ||
            blockhash(block.number-1)!=order.parent) revert WrongSlot();
        // The legacy entry point is external. Self-delegation preserves the original wallet and allowance.
        // Both the target and the single selector are fixed; the caller supplies only swap amounts and currencies.
        (bool ok, bytes memory result)=address(this).delegatecall(abi.encodeCall(
            PropVenueAdapter.execute, (market,order.tokenIn,order.tokenOut,order.amount,order.minimum,order.timestamp)
        ));
        if (!ok) {
            assembly ("memory-safe") { revert(add(result,32),mload(result)) }
        }
        spent=order.amount;
        received=abi.decode(result,(uint256));
        emit SlotFilled(order.leg,msg.sender,market,spent,received);
    }
}
