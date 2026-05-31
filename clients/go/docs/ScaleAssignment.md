# ScaleAssignment

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Indices** | **[]int32** | Replica indices within this role that the receiving node should ensure exist locally. Each becomes a container &#x60;{service}-{role}-{index}&#x60;. | 
**Role** | **string** | Role name (e.g. &#x60;\&quot;default\&quot;&#x60;, &#x60;\&quot;primary\&quot;&#x60;, &#x60;\&quot;read\&quot;&#x60;). | 

## Methods

### NewScaleAssignment

`func NewScaleAssignment(indices []int32, role string, ) *ScaleAssignment`

NewScaleAssignment instantiates a new ScaleAssignment object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewScaleAssignmentWithDefaults

`func NewScaleAssignmentWithDefaults() *ScaleAssignment`

NewScaleAssignmentWithDefaults instantiates a new ScaleAssignment object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetIndices

`func (o *ScaleAssignment) GetIndices() []int32`

GetIndices returns the Indices field if non-nil, zero value otherwise.

### GetIndicesOk

`func (o *ScaleAssignment) GetIndicesOk() (*[]int32, bool)`

GetIndicesOk returns a tuple with the Indices field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetIndices

`func (o *ScaleAssignment) SetIndices(v []int32)`

SetIndices sets Indices field to given value.


### GetRole

`func (o *ScaleAssignment) GetRole() string`

GetRole returns the Role field if non-nil, zero value otherwise.

### GetRoleOk

`func (o *ScaleAssignment) GetRoleOk() (*string, bool)`

GetRoleOk returns a tuple with the Role field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRole

`func (o *ScaleAssignment) SetRole(v string)`

SetRole sets Role field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


