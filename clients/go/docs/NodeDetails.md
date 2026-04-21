# NodeDetails

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Address** | **string** | Node network address | 
**Id** | **int64** | Node identifier | 
**Labels** | **map[string]string** | Node labels for scheduling | 
**LastHeartbeat** | **int64** | Last heartbeat timestamp (Unix timestamp) | 
**LastSeen** | **int64** | Last seen timestamp (Unix timestamp) | 
**RegisteredAt** | **int64** | When the node was registered (Unix timestamp) | 
**Resources** | [**NodeResourceInfo**](NodeResourceInfo.md) | Node resource information | 
**Role** | **string** | Node role | 
**Services** | **[]string** | Services running on this node | 
**Status** | **string** | Current node status | 

## Methods

### NewNodeDetails

`func NewNodeDetails(address string, id int64, labels map[string]string, lastHeartbeat int64, lastSeen int64, registeredAt int64, resources NodeResourceInfo, role string, services []string, status string, ) *NodeDetails`

NewNodeDetails instantiates a new NodeDetails object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNodeDetailsWithDefaults

`func NewNodeDetailsWithDefaults() *NodeDetails`

NewNodeDetailsWithDefaults instantiates a new NodeDetails object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAddress

`func (o *NodeDetails) GetAddress() string`

GetAddress returns the Address field if non-nil, zero value otherwise.

### GetAddressOk

`func (o *NodeDetails) GetAddressOk() (*string, bool)`

GetAddressOk returns a tuple with the Address field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAddress

`func (o *NodeDetails) SetAddress(v string)`

SetAddress sets Address field to given value.


### GetId

`func (o *NodeDetails) GetId() int64`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *NodeDetails) GetIdOk() (*int64, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *NodeDetails) SetId(v int64)`

SetId sets Id field to given value.


### GetLabels

`func (o *NodeDetails) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *NodeDetails) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *NodeDetails) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.


### GetLastHeartbeat

`func (o *NodeDetails) GetLastHeartbeat() int64`

GetLastHeartbeat returns the LastHeartbeat field if non-nil, zero value otherwise.

### GetLastHeartbeatOk

`func (o *NodeDetails) GetLastHeartbeatOk() (*int64, bool)`

GetLastHeartbeatOk returns a tuple with the LastHeartbeat field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastHeartbeat

`func (o *NodeDetails) SetLastHeartbeat(v int64)`

SetLastHeartbeat sets LastHeartbeat field to given value.


### GetLastSeen

`func (o *NodeDetails) GetLastSeen() int64`

GetLastSeen returns the LastSeen field if non-nil, zero value otherwise.

### GetLastSeenOk

`func (o *NodeDetails) GetLastSeenOk() (*int64, bool)`

GetLastSeenOk returns a tuple with the LastSeen field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastSeen

`func (o *NodeDetails) SetLastSeen(v int64)`

SetLastSeen sets LastSeen field to given value.


### GetRegisteredAt

`func (o *NodeDetails) GetRegisteredAt() int64`

GetRegisteredAt returns the RegisteredAt field if non-nil, zero value otherwise.

### GetRegisteredAtOk

`func (o *NodeDetails) GetRegisteredAtOk() (*int64, bool)`

GetRegisteredAtOk returns a tuple with the RegisteredAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRegisteredAt

`func (o *NodeDetails) SetRegisteredAt(v int64)`

SetRegisteredAt sets RegisteredAt field to given value.


### GetResources

`func (o *NodeDetails) GetResources() NodeResourceInfo`

GetResources returns the Resources field if non-nil, zero value otherwise.

### GetResourcesOk

`func (o *NodeDetails) GetResourcesOk() (*NodeResourceInfo, bool)`

GetResourcesOk returns a tuple with the Resources field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResources

`func (o *NodeDetails) SetResources(v NodeResourceInfo)`

SetResources sets Resources field to given value.


### GetRole

`func (o *NodeDetails) GetRole() string`

GetRole returns the Role field if non-nil, zero value otherwise.

### GetRoleOk

`func (o *NodeDetails) GetRoleOk() (*string, bool)`

GetRoleOk returns a tuple with the Role field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRole

`func (o *NodeDetails) SetRole(v string)`

SetRole sets Role field to given value.


### GetServices

`func (o *NodeDetails) GetServices() []string`

GetServices returns the Services field if non-nil, zero value otherwise.

### GetServicesOk

`func (o *NodeDetails) GetServicesOk() (*[]string, bool)`

GetServicesOk returns a tuple with the Services field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetServices

`func (o *NodeDetails) SetServices(v []string)`

SetServices sets Services field to given value.


### GetStatus

`func (o *NodeDetails) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *NodeDetails) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *NodeDetails) SetStatus(v string)`

SetStatus sets Status field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


