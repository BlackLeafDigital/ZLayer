# InternalScaleRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Assignments** | Pointer to [**[]ScaleAssignment**](ScaleAssignment.md) | Per-role-group container index lists. Empty in Phase 1; populated by Phase 2 once &#x60;replica_groups&#x60; + cross-node identity ship. | [optional] 
**Replicas** | Pointer to **int32** | Total target replica count for this node, when caller didn&#39;t supply explicit per-role assignments (legacy / Phase 1 shape). When &#x60;assignments&#x60; is non-empty, this field is informational. | [optional] 
**Service** | **string** | Service name. | 

## Methods

### NewInternalScaleRequest

`func NewInternalScaleRequest(service string, ) *InternalScaleRequest`

NewInternalScaleRequest instantiates a new InternalScaleRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewInternalScaleRequestWithDefaults

`func NewInternalScaleRequestWithDefaults() *InternalScaleRequest`

NewInternalScaleRequestWithDefaults instantiates a new InternalScaleRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAssignments

`func (o *InternalScaleRequest) GetAssignments() []ScaleAssignment`

GetAssignments returns the Assignments field if non-nil, zero value otherwise.

### GetAssignmentsOk

`func (o *InternalScaleRequest) GetAssignmentsOk() (*[]ScaleAssignment, bool)`

GetAssignmentsOk returns a tuple with the Assignments field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAssignments

`func (o *InternalScaleRequest) SetAssignments(v []ScaleAssignment)`

SetAssignments sets Assignments field to given value.

### HasAssignments

`func (o *InternalScaleRequest) HasAssignments() bool`

HasAssignments returns a boolean if a field has been set.

### GetReplicas

`func (o *InternalScaleRequest) GetReplicas() int32`

GetReplicas returns the Replicas field if non-nil, zero value otherwise.

### GetReplicasOk

`func (o *InternalScaleRequest) GetReplicasOk() (*int32, bool)`

GetReplicasOk returns a tuple with the Replicas field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReplicas

`func (o *InternalScaleRequest) SetReplicas(v int32)`

SetReplicas sets Replicas field to given value.

### HasReplicas

`func (o *InternalScaleRequest) HasReplicas() bool`

HasReplicas returns a boolean if a field has been set.

### GetService

`func (o *InternalScaleRequest) GetService() string`

GetService returns the Service field if non-nil, zero value otherwise.

### GetServiceOk

`func (o *InternalScaleRequest) GetServiceOk() (*string, bool)`

GetServiceOk returns a tuple with the Service field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetService

`func (o *InternalScaleRequest) SetService(v string)`

SetService sets Service field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


