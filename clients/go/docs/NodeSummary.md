# NodeSummary

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Address** | **string** | Node network address | 
**Id** | **int64** | Node identifier | 
**Labels** | **map[string]string** | Node labels for scheduling | 
**LastSeen** | **int64** | Last seen timestamp (Unix timestamp) | 
**Role** | **string** | Node role (e.g., \&quot;leader\&quot;, \&quot;worker\&quot;) | 
**Status** | **string** | Current node status (e.g., \&quot;ready\&quot;, \&quot;notready\&quot;, \&quot;disconnected\&quot;) | 

## Methods

### NewNodeSummary

`func NewNodeSummary(address string, id int64, labels map[string]string, lastSeen int64, role string, status string, ) *NodeSummary`

NewNodeSummary instantiates a new NodeSummary object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNodeSummaryWithDefaults

`func NewNodeSummaryWithDefaults() *NodeSummary`

NewNodeSummaryWithDefaults instantiates a new NodeSummary object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAddress

`func (o *NodeSummary) GetAddress() string`

GetAddress returns the Address field if non-nil, zero value otherwise.

### GetAddressOk

`func (o *NodeSummary) GetAddressOk() (*string, bool)`

GetAddressOk returns a tuple with the Address field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAddress

`func (o *NodeSummary) SetAddress(v string)`

SetAddress sets Address field to given value.


### GetId

`func (o *NodeSummary) GetId() int64`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *NodeSummary) GetIdOk() (*int64, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *NodeSummary) SetId(v int64)`

SetId sets Id field to given value.


### GetLabels

`func (o *NodeSummary) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *NodeSummary) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *NodeSummary) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.


### GetLastSeen

`func (o *NodeSummary) GetLastSeen() int64`

GetLastSeen returns the LastSeen field if non-nil, zero value otherwise.

### GetLastSeenOk

`func (o *NodeSummary) GetLastSeenOk() (*int64, bool)`

GetLastSeenOk returns a tuple with the LastSeen field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastSeen

`func (o *NodeSummary) SetLastSeen(v int64)`

SetLastSeen sets LastSeen field to given value.


### GetRole

`func (o *NodeSummary) GetRole() string`

GetRole returns the Role field if non-nil, zero value otherwise.

### GetRoleOk

`func (o *NodeSummary) GetRoleOk() (*string, bool)`

GetRoleOk returns a tuple with the Role field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRole

`func (o *NodeSummary) SetRole(v string)`

SetRole sets Role field to given value.


### GetStatus

`func (o *NodeSummary) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *NodeSummary) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *NodeSummary) SetStatus(v string)`

SetStatus sets Status field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


