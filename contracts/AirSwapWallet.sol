// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Original V4/V5 cash ABI: https://github.com/airswap/airswap-protocols/blob/a117f69c2d0da18ff08500cd0b41ea8abe92856a/source/swap-erc20/contracts/interfaces/ISwapERC20.sol
interface IAirSwapCash {
    function swap(address,uint256,uint256,address,address,uint256,address,uint256,uint8,bytes32,bytes32) external;
    function swapAnySender(address,uint256,uint256,address,address,uint256,address,uint256,uint8,bytes32,bytes32) external;
}
interface IAirSwapToken {
    function balanceOf(address) external view returns(uint256);
    function allowance(address,address) external view returns(uint256);
    function transferFrom(address,address,uint256) external returns(bool);
    function transfer(address,uint256) external returns(bool);
    function approve(address,uint256) external returns(bool);
}

contract AirSwapWallet {
    // Stored once in the constructor so the verified runtime is identical for
    // all owners of a given exchange. No owner change, arbitrary call or sweep.
    address public owner;
    address public immutable exchange;
    bytes32 public immutable exchangeHash;
    bool private busy;

    struct Order {
        uint256 nonce;
        uint256 expiry;
        address maker;
        address input;
        uint256 amount;
        address output;
        uint256 quoted;
        bool anySender;
        uint8 v;
        bytes32 r;
        bytes32 s;
    }
    event Paid(address indexed payer,address indexed maker,uint256 indexed nonce,address input,address output,uint256 spent,uint256 received);
    error InvalidOrder();
    error Payment();

    constructor(address venue) {
        if(venue.code.length==0) revert InvalidOrder();
        owner=msg.sender;
        exchange=venue;
        exchangeHash=venue.codehash;
    }

    function invoke(address asset,bytes memory data) private {
        (bool ok,bytes memory result)=asset.call(data);
        if(!ok || (result.length!=0 && !abi.decode(result,(bool)))) revert Payment();
    }
    function approve(IAirSwapToken asset,uint256 amount) private {
        invoke(address(asset),abi.encodeCall(asset.approve,(exchange,amount)));
        if(asset.allowance(address(this),exchange)!=amount) revert Payment();
    }
    function execute(Order calldata q,uint256 minimum,uint256 deadline) external returns(uint256 spent,uint256 received) {
        if(busy || msg.sender!=owner || q.maker==address(0) || q.maker==owner || q.maker==address(this) ||
           q.input==q.output || q.amount==0 || q.quoted==0 || minimum==0 ||
           block.timestamp>deadline || block.timestamp>=q.expiry || exchange.codehash!=exchangeHash) revert InvalidOrder();
        busy=true;
        IAirSwapToken token=IAirSwapToken(q.input);
        uint256 start=token.balanceOf(address(this));
        uint256 debit=token.balanceOf(owner);
        uint256 credit=IAirSwapToken(q.output).balanceOf(owner);
        invoke(q.input,abi.encodeCall(token.transferFrom,(owner,address(this),q.amount)));
        if(token.balanceOf(address(this))-start<q.amount) revert Payment();
        if(token.allowance(address(this),exchange)!=0) approve(token,0);
        approve(token,q.amount);
        bytes4 method=q.anySender ? IAirSwapCash.swapAnySender.selector : IAirSwapCash.swap.selector;
        (bool success,bytes memory reason)=exchange.call(abi.encodeWithSelector(method,owner,q.nonce,q.expiry,q.maker,
            q.output,q.quoted,q.input,q.amount,q.v,q.r,q.s));
        if(!success) assembly ("memory-safe") { revert(add(reason,32),mload(reason)) }
        if(token.allowance(address(this),exchange)!=0) approve(token,0);
        uint256 leftover=token.balanceOf(address(this))-start;
        if(leftover!=0) invoke(q.input,abi.encodeCall(token.transfer,(owner,leftover)));
        spent=debit-token.balanceOf(owner);
        received=IAirSwapToken(q.output).balanceOf(owner)-credit;
        if(spent==0 || spent>q.amount || received<minimum || token.balanceOf(address(this))!=start) revert Payment();
        emit Paid(owner,q.maker,q.nonce,q.input,q.output,spent,received);
        busy=false;
    }
}
