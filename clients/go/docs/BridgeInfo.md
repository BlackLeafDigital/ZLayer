# BridgeInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Gateway** | **string** | Gateway IP within the subnet (first usable address); the bridge&#39;s own L3 address. Containers&#39; default route points here. | 
**Name** | **string** | Linux bridge name (&#x60;br-svc-&lt;hash&gt;&#x60;), max 15 chars per IFNAMSIZ. | 
**NodeId** | **string** | Node identifier (stringified to avoid leaking &#x60;NodeId&#x60; type details across crate boundaries). | 
**Subnet** | **string** | CIDR of the service subnet assigned to this node. | 

## Methods

### NewBridgeInfo

`func NewBridgeInfo(gateway string, name string, nodeId string, subnet string, ) *BridgeInfo`

NewBridgeInfo instantiates a new BridgeInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewBridgeInfoWithDefaults

`func NewBridgeInfoWithDefaults() *BridgeInfo`

NewBridgeInfoWithDefaults instantiates a new BridgeInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetGateway

`func (o *BridgeInfo) GetGateway() string`

GetGateway returns the Gateway field if non-nil, zero value otherwise.

### GetGatewayOk

`func (o *BridgeInfo) GetGatewayOk() (*string, bool)`

GetGatewayOk returns a tuple with the Gateway field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGateway

`func (o *BridgeInfo) SetGateway(v string)`

SetGateway sets Gateway field to given value.


### GetName

`func (o *BridgeInfo) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *BridgeInfo) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *BridgeInfo) SetName(v string)`

SetName sets Name field to given value.


### GetNodeId

`func (o *BridgeInfo) GetNodeId() string`

GetNodeId returns the NodeId field if non-nil, zero value otherwise.

### GetNodeIdOk

`func (o *BridgeInfo) GetNodeIdOk() (*string, bool)`

GetNodeIdOk returns a tuple with the NodeId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNodeId

`func (o *BridgeInfo) SetNodeId(v string)`

SetNodeId sets NodeId field to given value.


### GetSubnet

`func (o *BridgeInfo) GetSubnet() string`

GetSubnet returns the Subnet field if non-nil, zero value otherwise.

### GetSubnetOk

`func (o *BridgeInfo) GetSubnetOk() (*string, bool)`

GetSubnetOk returns a tuple with the Subnet field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSubnet

`func (o *BridgeInfo) SetSubnet(v string)`

SetSubnet sets Subnet field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


