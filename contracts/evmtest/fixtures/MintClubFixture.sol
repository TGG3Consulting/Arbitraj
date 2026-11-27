// SPDX-License-Identifier: MIT
// Изолированный тестовый эмитент для проверки границ собственного MintClubAdapter с TaxToken. К сети не подключается.
pragma solidity 0.8.37;

interface ITestReserve {
    function transfer(address to,uint256 amount) external returns(bool);
    function transferFrom(address from,address to,uint256 amount) external returns(bool);
}
contract TestIssued {
    address public immutable issuer;
    mapping(address=>uint256) public balanceOf;
    mapping(address=>mapping(address=>uint256)) public allowance;
    constructor(address registry){issuer=registry;}
    function issue(address to,uint256 qty) external {require(msg.sender==issuer);balanceOf[to]+=qty;}
    function redeem(address owner,uint256 qty) external {require(msg.sender==issuer);allowance[owner][issuer]-=qty;balanceOf[owner]-=qty;}
    function approve(address spender,uint256 amount) external returns(bool){allowance[msg.sender][spender]=amount;return true;}
    function transferFrom(address from,address to,uint256 amount) external returns(bool){
        allowance[from][msg.sender]-=amount;balanceOf[from]-=amount;balanceOf[to]+=amount;return true;
    }
}
contract MintClubFixture {
    TestIssued public immutable implementation;
    TestIssued public immutable issued;
    ITestReserve public immutable reserve;
    constructor(ITestReserve asset){
        reserve=asset;implementation=new TestIssued(address(this));
        bytes memory init=abi.encodePacked(hex"3d602d80600a3d3981f3363d3d373d3d3d363d73",address(implementation),hex"5af43d82803e903d91602b57fd5bf3");
        address clone;
        assembly("memory-safe"){clone:=create(0,add(init,32),mload(init))}
        require(clone.code.length==45);issued=TestIssued(clone);
    }
    function tokenBond(address market) external view returns(address,uint16,uint16,uint40,address,uint256){
        require(market==address(issued));return(address(this),0,0,0,address(reserve),0);
    }
    function getReserveForToken(address,uint256 qty) external pure returns(uint256,uint256){return(qty,0);}
    function mint(address market,uint256 qty,uint256 cap,address to) external returns(uint256){
        require(market==address(issued) && qty<=cap);issued.issue(to,qty);require(reserve.transferFrom(msg.sender,address(this),qty));return qty;
    }
    function burn(address market,uint256 qty,uint256 floor,address to) external returns(uint256){
        require(market==address(issued) && qty>=floor);issued.redeem(msg.sender,qty);require(reserve.transfer(to,qty));return qty;
    }
}
