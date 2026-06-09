# ConnectBridgeNetworkRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Aliases** | Pointer to **[]string** | Optional DNS aliases on this network. | [optional] 
**ContainerId** | **string** | Container id to attach. | 
**Ipv4Address** | Pointer to **NullableString** | Optional static IPv4 to pin this container to. Validated as [&#x60;std::net::Ipv4Addr&#x60;]. | [optional] 

## Methods

### NewConnectBridgeNetworkRequest

`func NewConnectBridgeNetworkRequest(containerId string, ) *ConnectBridgeNetworkRequest`

NewConnectBridgeNetworkRequest instantiates a new ConnectBridgeNetworkRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewConnectBridgeNetworkRequestWithDefaults

`func NewConnectBridgeNetworkRequestWithDefaults() *ConnectBridgeNetworkRequest`

NewConnectBridgeNetworkRequestWithDefaults instantiates a new ConnectBridgeNetworkRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAliases

`func (o *ConnectBridgeNetworkRequest) GetAliases() []string`

GetAliases returns the Aliases field if non-nil, zero value otherwise.

### GetAliasesOk

`func (o *ConnectBridgeNetworkRequest) GetAliasesOk() (*[]string, bool)`

GetAliasesOk returns a tuple with the Aliases field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAliases

`func (o *ConnectBridgeNetworkRequest) SetAliases(v []string)`

SetAliases sets Aliases field to given value.

### HasAliases

`func (o *ConnectBridgeNetworkRequest) HasAliases() bool`

HasAliases returns a boolean if a field has been set.

### GetContainerId

`func (o *ConnectBridgeNetworkRequest) GetContainerId() string`

GetContainerId returns the ContainerId field if non-nil, zero value otherwise.

### GetContainerIdOk

`func (o *ConnectBridgeNetworkRequest) GetContainerIdOk() (*string, bool)`

GetContainerIdOk returns a tuple with the ContainerId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetContainerId

`func (o *ConnectBridgeNetworkRequest) SetContainerId(v string)`

SetContainerId sets ContainerId field to given value.


### GetIpv4Address

`func (o *ConnectBridgeNetworkRequest) GetIpv4Address() string`

GetIpv4Address returns the Ipv4Address field if non-nil, zero value otherwise.

### GetIpv4AddressOk

`func (o *ConnectBridgeNetworkRequest) GetIpv4AddressOk() (*string, bool)`

GetIpv4AddressOk returns a tuple with the Ipv4Address field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetIpv4Address

`func (o *ConnectBridgeNetworkRequest) SetIpv4Address(v string)`

SetIpv4Address sets Ipv4Address field to given value.

### HasIpv4Address

`func (o *ConnectBridgeNetworkRequest) HasIpv4Address() bool`

HasIpv4Address returns a boolean if a field has been set.

### SetIpv4AddressNil

`func (o *ConnectBridgeNetworkRequest) SetIpv4AddressNil(b bool)`

 SetIpv4AddressNil sets the value for Ipv4Address to be an explicit nil

### UnsetIpv4Address
`func (o *ConnectBridgeNetworkRequest) UnsetIpv4Address()`

UnsetIpv4Address ensures that no value is present for Ipv4Address, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


