# OverlayStatusResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Cidr** | **string** | Overlay network CIDR | 
**HealthyPeers** | **int32** | Number of healthy peers | 
**Interface** | **string** | Overlay interface name | 
**IsLeader** | **bool** | Whether this node is the cluster leader | 
**LastCheck** | **int64** | Last health check timestamp (unix epoch seconds) | 
**NodeIp** | **string** | Node&#39;s overlay IP address | 
**Port** | **int32** | Overlay listen port (&#x60;WireGuard&#x60; protocol) | 
**TotalPeers** | **int32** | Total number of peers | 
**UnhealthyPeers** | **int32** | Number of unhealthy peers | 

## Methods

### NewOverlayStatusResponse

`func NewOverlayStatusResponse(cidr string, healthyPeers int32, interface_ string, isLeader bool, lastCheck int64, nodeIp string, port int32, totalPeers int32, unhealthyPeers int32, ) *OverlayStatusResponse`

NewOverlayStatusResponse instantiates a new OverlayStatusResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewOverlayStatusResponseWithDefaults

`func NewOverlayStatusResponseWithDefaults() *OverlayStatusResponse`

NewOverlayStatusResponseWithDefaults instantiates a new OverlayStatusResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCidr

`func (o *OverlayStatusResponse) GetCidr() string`

GetCidr returns the Cidr field if non-nil, zero value otherwise.

### GetCidrOk

`func (o *OverlayStatusResponse) GetCidrOk() (*string, bool)`

GetCidrOk returns a tuple with the Cidr field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCidr

`func (o *OverlayStatusResponse) SetCidr(v string)`

SetCidr sets Cidr field to given value.


### GetHealthyPeers

`func (o *OverlayStatusResponse) GetHealthyPeers() int32`

GetHealthyPeers returns the HealthyPeers field if non-nil, zero value otherwise.

### GetHealthyPeersOk

`func (o *OverlayStatusResponse) GetHealthyPeersOk() (*int32, bool)`

GetHealthyPeersOk returns a tuple with the HealthyPeers field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHealthyPeers

`func (o *OverlayStatusResponse) SetHealthyPeers(v int32)`

SetHealthyPeers sets HealthyPeers field to given value.


### GetInterface

`func (o *OverlayStatusResponse) GetInterface() string`

GetInterface returns the Interface field if non-nil, zero value otherwise.

### GetInterfaceOk

`func (o *OverlayStatusResponse) GetInterfaceOk() (*string, bool)`

GetInterfaceOk returns a tuple with the Interface field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetInterface

`func (o *OverlayStatusResponse) SetInterface(v string)`

SetInterface sets Interface field to given value.


### GetIsLeader

`func (o *OverlayStatusResponse) GetIsLeader() bool`

GetIsLeader returns the IsLeader field if non-nil, zero value otherwise.

### GetIsLeaderOk

`func (o *OverlayStatusResponse) GetIsLeaderOk() (*bool, bool)`

GetIsLeaderOk returns a tuple with the IsLeader field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetIsLeader

`func (o *OverlayStatusResponse) SetIsLeader(v bool)`

SetIsLeader sets IsLeader field to given value.


### GetLastCheck

`func (o *OverlayStatusResponse) GetLastCheck() int64`

GetLastCheck returns the LastCheck field if non-nil, zero value otherwise.

### GetLastCheckOk

`func (o *OverlayStatusResponse) GetLastCheckOk() (*int64, bool)`

GetLastCheckOk returns a tuple with the LastCheck field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastCheck

`func (o *OverlayStatusResponse) SetLastCheck(v int64)`

SetLastCheck sets LastCheck field to given value.


### GetNodeIp

`func (o *OverlayStatusResponse) GetNodeIp() string`

GetNodeIp returns the NodeIp field if non-nil, zero value otherwise.

### GetNodeIpOk

`func (o *OverlayStatusResponse) GetNodeIpOk() (*string, bool)`

GetNodeIpOk returns a tuple with the NodeIp field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNodeIp

`func (o *OverlayStatusResponse) SetNodeIp(v string)`

SetNodeIp sets NodeIp field to given value.


### GetPort

`func (o *OverlayStatusResponse) GetPort() int32`

GetPort returns the Port field if non-nil, zero value otherwise.

### GetPortOk

`func (o *OverlayStatusResponse) GetPortOk() (*int32, bool)`

GetPortOk returns a tuple with the Port field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPort

`func (o *OverlayStatusResponse) SetPort(v int32)`

SetPort sets Port field to given value.


### GetTotalPeers

`func (o *OverlayStatusResponse) GetTotalPeers() int32`

GetTotalPeers returns the TotalPeers field if non-nil, zero value otherwise.

### GetTotalPeersOk

`func (o *OverlayStatusResponse) GetTotalPeersOk() (*int32, bool)`

GetTotalPeersOk returns a tuple with the TotalPeers field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTotalPeers

`func (o *OverlayStatusResponse) SetTotalPeers(v int32)`

SetTotalPeers sets TotalPeers field to given value.


### GetUnhealthyPeers

`func (o *OverlayStatusResponse) GetUnhealthyPeers() int32`

GetUnhealthyPeers returns the UnhealthyPeers field if non-nil, zero value otherwise.

### GetUnhealthyPeersOk

`func (o *OverlayStatusResponse) GetUnhealthyPeersOk() (*int32, bool)`

GetUnhealthyPeersOk returns a tuple with the UnhealthyPeers field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUnhealthyPeers

`func (o *OverlayStatusResponse) SetUnhealthyPeers(v int32)`

SetUnhealthyPeers sets UnhealthyPeers field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


