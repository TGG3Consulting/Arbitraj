// SPDX-License-Identifier: MIT
// Выпуск и погашение ERC20 Mint Club за счёт отправителя с атомарными пределами кошелька и проверкой резерва Bond.
pragma solidity 0.8.37;

interface IMintClubBond {
    function tokenBond(address token) external view returns(address,uint16,uint16,uint40,address,uint256);
    function getReserveForToken(address token,uint256 qty) external view returns(uint256,uint256);
    function mint(address token,uint256 qty,uint256 maxCost,address receiver) external returns(uint256);
    function burn(address token,uint256 qty,uint256 minRefund,address receiver) external returns(uint256);
}
interface IMintClubAsset {
    function balanceOf(address account) external view returns(uint256);
    function allowance(address account,address spender) external view returns(uint256);
    function transferFrom(address from,address to,uint256 amount) external returns(bool);
    function approve(address spender,uint256 amount) external returns(bool);
}

contract MintClubAdapter {
    IMintClubBond public immutable bond;
    bytes32 public immutable cloneHash;
    bool transient private entered;
    event Settled(address indexed payer,address indexed token,address indexed reserve,bool mint,uint256 qty,uint256 debit,uint256 credit);

    constructor(IMintClubBond registry,address implementation){
        require(address(registry).code.length!=0 && implementation.code.length!=0,"issuer");
        bond=registry;
        cloneHash=keccak256(abi.encodePacked(hex"363d3d373d3d3d363d73",implementation,hex"5af43d82803e903d91602b57fd5bf3"));
    }
    function tokenCall(address token,bytes memory input) private{
        (bool ok,bytes memory out)=token.call(input);
        require(ok && (out.length==0 || abi.decode(out,(bool))),"erc20");
    }
    // Вызов погашения оплачивается только токенами заявителя. Публичный остаток модуля не используется как вход.
    function execute(address token,address reserve,bool mint,uint256 qty,uint256 maxDebit,uint256 minCredit,uint256 deadline)
        external returns(uint256 debit,uint256 credit){
        require(!entered && qty!=0 && maxDebit!=0 && minCredit!=0 && block.timestamp<=deadline,"order");
        entered=true;
        (,,,,address registered,)=bond.tokenBond(token);
        require(token.codehash==cloneHash && reserve==registered && token!=reserve && reserve.code.length!=0 &&
                msg.sender!=address(bond) && msg.sender!=address(this),"market");
        (address input,address output)=mint?(reserve,token):(token,reserve);
        IMintClubAsset asset=IMintClubAsset(input);
        uint256 oldIn=asset.balanceOf(msg.sender);
        uint256 oldOut=IMintClubAsset(output).balanceOf(msg.sender);
        uint256 local=asset.balanceOf(address(this));
        uint256 physical=IMintClubAsset(reserve).balanceOf(address(bond));
        uint256 pay=qty;
        if(mint) (pay,)=bond.getReserveForToken(token,qty);
        require(pay!=0 && pay<=maxDebit,"price");
        tokenCall(input,abi.encodeCall(IMintClubAsset.transferFrom,(msg.sender,address(this),pay)));
        require(asset.balanceOf(address(this))-local==pay,"funding");
        tokenCall(input,abi.encodeCall(IMintClubAsset.approve,(address(bond),pay)));
        if(mint){
            require(bond.mint(token,qty,pay,msg.sender)==pay,"cost");
            require(IMintClubAsset(reserve).balanceOf(address(bond))-physical==pay,"bond payment");
        }else{
            uint256 refund=bond.burn(token,qty,minCredit,msg.sender);
            // Исходящая комиссия может уменьшать кредит получателя; нельзя оплачивать чужие расходы общим резервом.
            require(physical-IMintClubAsset(reserve).balanceOf(address(bond))==refund,"bond debit");
        }
        if(asset.allowance(address(this),address(bond))!=0){
            tokenCall(input,abi.encodeCall(IMintClubAsset.approve,(address(bond),0)));
            require(asset.allowance(address(this),address(bond))==0,"allowance");
        }
        debit=oldIn-asset.balanceOf(msg.sender);
        credit=IMintClubAsset(output).balanceOf(msg.sender)-oldOut;
        require(asset.balanceOf(address(this))==local && debit>0 && debit<=maxDebit && credit>=minCredit,"wallet");
        require(mint?credit==qty:debit==qty,"quantity");
        entered=false;
        emit Settled(msg.sender,token,reserve,mint,qty,debit,credit);
    }
}
