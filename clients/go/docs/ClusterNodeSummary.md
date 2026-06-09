# ClusterNodeSummary

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Address** | **string** | Network address (Raft RPC address) | 
**AdvertiseAddr** | **string** | Advertise address (public IP) | 
**ApiEndpoint** | Pointer to **string** | API endpoint as &#x60;advertise_addr:api_port&#x60; (e.g., \&quot;127.0.0.1:19110\&quot;). Distinct from &#x60;address&#x60; which holds the Raft RPC endpoint. | [optional] 
**CpuTotal** | **float64** | Total CPU cores on this node | 
**CpuUsed** | **float64** | Current CPU usage (cores) | 
**Id** | **string** | UUID or Raft-level ID | 
**IsLeader** | **bool** | Whether this node is the Raft leader | 
**LastHeartbeat** | **int64** | Last heartbeat timestamp (Unix timestamp ms) | 
**MemoryTotal** | **int64** | Total memory in bytes | 
**MemoryUsed** | **int64** | Current memory usage in bytes | 
**Mode** | **string** | Join mode: \&quot;full\&quot; or \&quot;replicate\&quot; | 
**OverlayIp** | **string** | Overlay network IP assigned to this node | 
**RegisteredAt** | **int64** | When the node was registered (Unix timestamp ms) | 
**Role** | **string** | Role in the Raft cluster: \&quot;leader\&quot;, \&quot;voter\&quot;, or \&quot;learner\&quot; | 
**Status** | **string** | Current status (e.g. \&quot;ready\&quot;, \&quot;draining\&quot;, \&quot;dead\&quot;) | 

## Methods

### NewClusterNodeSummary

`func NewClusterNodeSummary(address string, advertiseAddr string, cpuTotal float64, cpuUsed float64, id string, isLeader bool, lastHeartbeat int64, memoryTotal int64, memoryUsed int64, mode string, overlayIp string, registeredAt int64, role string, status string, ) *ClusterNodeSummary`

NewClusterNodeSummary instantiates a new ClusterNodeSummary object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewClusterNodeSummaryWithDefaults

`func NewClusterNodeSummaryWithDefaults() *ClusterNodeSummary`

NewClusterNodeSummaryWithDefaults instantiates a new ClusterNodeSummary object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAddress

`func (o *ClusterNodeSummary) GetAddress() string`

GetAddress returns the Address field if non-nil, zero value otherwise.

### GetAddressOk

`func (o *ClusterNodeSummary) GetAddressOk() (*string, bool)`

GetAddressOk returns a tuple with the Address field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAddress

`func (o *ClusterNodeSummary) SetAddress(v string)`

SetAddress sets Address field to given value.


### GetAdvertiseAddr

`func (o *ClusterNodeSummary) GetAdvertiseAddr() string`

GetAdvertiseAddr returns the AdvertiseAddr field if non-nil, zero value otherwise.

### GetAdvertiseAddrOk

`func (o *ClusterNodeSummary) GetAdvertiseAddrOk() (*string, bool)`

GetAdvertiseAddrOk returns a tuple with the AdvertiseAddr field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAdvertiseAddr

`func (o *ClusterNodeSummary) SetAdvertiseAddr(v string)`

SetAdvertiseAddr sets AdvertiseAddr field to given value.


### GetApiEndpoint

`func (o *ClusterNodeSummary) GetApiEndpoint() string`

GetApiEndpoint returns the ApiEndpoint field if non-nil, zero value otherwise.

### GetApiEndpointOk

`func (o *ClusterNodeSummary) GetApiEndpointOk() (*string, bool)`

GetApiEndpointOk returns a tuple with the ApiEndpoint field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetApiEndpoint

`func (o *ClusterNodeSummary) SetApiEndpoint(v string)`

SetApiEndpoint sets ApiEndpoint field to given value.

### HasApiEndpoint

`func (o *ClusterNodeSummary) HasApiEndpoint() bool`

HasApiEndpoint returns a boolean if a field has been set.

### GetCpuTotal

`func (o *ClusterNodeSummary) GetCpuTotal() float64`

GetCpuTotal returns the CpuTotal field if non-nil, zero value otherwise.

### GetCpuTotalOk

`func (o *ClusterNodeSummary) GetCpuTotalOk() (*float64, bool)`

GetCpuTotalOk returns a tuple with the CpuTotal field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuTotal

`func (o *ClusterNodeSummary) SetCpuTotal(v float64)`

SetCpuTotal sets CpuTotal field to given value.


### GetCpuUsed

`func (o *ClusterNodeSummary) GetCpuUsed() float64`

GetCpuUsed returns the CpuUsed field if non-nil, zero value otherwise.

### GetCpuUsedOk

`func (o *ClusterNodeSummary) GetCpuUsedOk() (*float64, bool)`

GetCpuUsedOk returns a tuple with the CpuUsed field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuUsed

`func (o *ClusterNodeSummary) SetCpuUsed(v float64)`

SetCpuUsed sets CpuUsed field to given value.


### GetId

`func (o *ClusterNodeSummary) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *ClusterNodeSummary) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *ClusterNodeSummary) SetId(v string)`

SetId sets Id field to given value.


### GetIsLeader

`func (o *ClusterNodeSummary) GetIsLeader() bool`

GetIsLeader returns the IsLeader field if non-nil, zero value otherwise.

### GetIsLeaderOk

`func (o *ClusterNodeSummary) GetIsLeaderOk() (*bool, bool)`

GetIsLeaderOk returns a tuple with the IsLeader field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetIsLeader

`func (o *ClusterNodeSummary) SetIsLeader(v bool)`

SetIsLeader sets IsLeader field to given value.


### GetLastHeartbeat

`func (o *ClusterNodeSummary) GetLastHeartbeat() int64`

GetLastHeartbeat returns the LastHeartbeat field if non-nil, zero value otherwise.

### GetLastHeartbeatOk

`func (o *ClusterNodeSummary) GetLastHeartbeatOk() (*int64, bool)`

GetLastHeartbeatOk returns a tuple with the LastHeartbeat field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastHeartbeat

`func (o *ClusterNodeSummary) SetLastHeartbeat(v int64)`

SetLastHeartbeat sets LastHeartbeat field to given value.


### GetMemoryTotal

`func (o *ClusterNodeSummary) GetMemoryTotal() int64`

GetMemoryTotal returns the MemoryTotal field if non-nil, zero value otherwise.

### GetMemoryTotalOk

`func (o *ClusterNodeSummary) GetMemoryTotalOk() (*int64, bool)`

GetMemoryTotalOk returns a tuple with the MemoryTotal field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryTotal

`func (o *ClusterNodeSummary) SetMemoryTotal(v int64)`

SetMemoryTotal sets MemoryTotal field to given value.


### GetMemoryUsed

`func (o *ClusterNodeSummary) GetMemoryUsed() int64`

GetMemoryUsed returns the MemoryUsed field if non-nil, zero value otherwise.

### GetMemoryUsedOk

`func (o *ClusterNodeSummary) GetMemoryUsedOk() (*int64, bool)`

GetMemoryUsedOk returns a tuple with the MemoryUsed field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryUsed

`func (o *ClusterNodeSummary) SetMemoryUsed(v int64)`

SetMemoryUsed sets MemoryUsed field to given value.


### GetMode

`func (o *ClusterNodeSummary) GetMode() string`

GetMode returns the Mode field if non-nil, zero value otherwise.

### GetModeOk

`func (o *ClusterNodeSummary) GetModeOk() (*string, bool)`

GetModeOk returns a tuple with the Mode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMode

`func (o *ClusterNodeSummary) SetMode(v string)`

SetMode sets Mode field to given value.


### GetOverlayIp

`func (o *ClusterNodeSummary) GetOverlayIp() string`

GetOverlayIp returns the OverlayIp field if non-nil, zero value otherwise.

### GetOverlayIpOk

`func (o *ClusterNodeSummary) GetOverlayIpOk() (*string, bool)`

GetOverlayIpOk returns a tuple with the OverlayIp field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOverlayIp

`func (o *ClusterNodeSummary) SetOverlayIp(v string)`

SetOverlayIp sets OverlayIp field to given value.


### GetRegisteredAt

`func (o *ClusterNodeSummary) GetRegisteredAt() int64`

GetRegisteredAt returns the RegisteredAt field if non-nil, zero value otherwise.

### GetRegisteredAtOk

`func (o *ClusterNodeSummary) GetRegisteredAtOk() (*int64, bool)`

GetRegisteredAtOk returns a tuple with the RegisteredAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRegisteredAt

`func (o *ClusterNodeSummary) SetRegisteredAt(v int64)`

SetRegisteredAt sets RegisteredAt field to given value.


### GetRole

`func (o *ClusterNodeSummary) GetRole() string`

GetRole returns the Role field if non-nil, zero value otherwise.

### GetRoleOk

`func (o *ClusterNodeSummary) GetRoleOk() (*string, bool)`

GetRoleOk returns a tuple with the Role field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRole

`func (o *ClusterNodeSummary) SetRole(v string)`

SetRole sets Role field to given value.


### GetStatus

`func (o *ClusterNodeSummary) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *ClusterNodeSummary) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *ClusterNodeSummary) SetStatus(v string)`

SetStatus sets Status field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


