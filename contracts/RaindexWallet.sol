// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Расчёт отдельной заявки Raindex из средств вызывающего кошелька.
// Адрес книги, хеш её кода и селектор официальной takeOrders закрепляются при размещении владельцем.
interface IRaindexAsset {
    function balanceOf(address) external view returns (uint256);
    function allowance(address,address) external view returns (uint256);
    function approve(address,uint256) external returns(bool);
    function transfer(address,uint256) external returns(bool);
    function transferFrom(address,address,uint256) external returns(bool);
}

contract RaindexWallet {
    address public immutable book;
    bytes32 public immutable bookCodeHash;
    bytes4 public immutable takeSelector;
    bool transient private entered;

    error InvalidBook();
    error Payment();
    event Settled(address indexed payer,bytes32 indexed orderCall,address indexed input,address output,uint256 spent,uint256 received);

    constructor(address book_,bytes32 codeHash_,bytes4 takeSelector_) {
        // IOrderBook V3..V5 и публичные IRaindex/IOrderBook V6,V7 соответственно.
        if(book_.code.length==0 || book_.codehash!=codeHash_ ||
           (takeSelector_!=0x8a44689c && takeSelector_!=0x0997c4a0 && takeSelector_!=0xd0f7a862 &&
            takeSelector_!=0x69c72856 && takeSelector_!=0xb0460f07)) revert InvalidBook();
        book=book_; bookCodeHash=codeHash_; takeSelector=takeSelector_;
    }

    struct Limits {address input; address output; uint256 minimumDebit; uint256 maximumDebit; uint256 minimumCredit; uint256 deadline;}

    function fill(Limits calldata l,bytes calldata action) external returns(uint256 spent,uint256 received) {
        if (book.codehash!=bookCodeHash || action.length<4 || bytes4(action[:4])!=takeSelector) revert InvalidBook();
        if (entered || block.timestamp>l.deadline || l.minimumDebit==0 || l.minimumDebit>l.maximumDebit ||
            l.minimumCredit==0 || l.input==l.output || l.input.code.length==0 || l.output.code.length==0) revert Payment();
        entered=true;
        IRaindexAsset input=IRaindexAsset(l.input);
        IRaindexAsset output=IRaindexAsset(l.output);
        uint256 heldIn=input.balanceOf(address(this));
        uint256 heldOut=output.balanceOf(address(this));
        uint256 ownerIn=input.balanceOf(msg.sender);
        uint256 ownerOut=output.balanceOf(msg.sender);
        if(input.allowance(address(this),book)!=0) revert Payment();
        transfer(l.input,abi.encodeCall(input.transferFrom,(msg.sender,address(this),l.maximumDebit)));
        if(input.balanceOf(address(this))!=heldIn+l.maximumDebit) revert Payment();
        transfer(l.input,abi.encodeCall(input.approve,(book,l.maximumDebit)));
        (bool ok,)=book.call(action);
        if(!ok) revert Payment();
        if(input.allowance(address(this),book)!=0) transfer(l.input,abi.encodeCall(input.approve,(book,0)));
        uint256 change=input.balanceOf(address(this))-heldIn;
        spent=l.maximumDebit-change;
        if(spent<l.minimumDebit) revert Payment();
        if(change!=0) transfer(l.input,abi.encodeCall(input.transfer,(msg.sender,change)));
        uint256 credit=output.balanceOf(address(this))-heldOut;
        if(credit<l.minimumCredit) revert Payment();
        transfer(l.output,abi.encodeCall(output.transfer,(msg.sender,credit)));
        received=output.balanceOf(msg.sender)-ownerOut;
        if(received<l.minimumCredit || ownerIn-input.balanceOf(msg.sender)!=spent ||
           input.balanceOf(address(this))!=heldIn || output.balanceOf(address(this))!=heldOut ||
           input.allowance(address(this),book)!=0) revert Payment();
        emit Settled(msg.sender,keccak256(action),l.input,l.output,spent,received);
        entered=false;
    }
    function transfer(address asset,bytes memory payload) private {
        (bool success,bytes memory ret)=asset.call(payload);
        if(!success || (ret.length!=0 && !abi.decode(ret,(bool)))) revert Payment();
    }
}
