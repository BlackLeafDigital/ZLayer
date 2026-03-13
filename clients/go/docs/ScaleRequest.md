# ScaleRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Replicas** | **int32** | Target replica count | 

## Methods

### NewScaleRequest

`func NewScaleRequest(replicas int32, ) *ScaleRequest`

NewScaleRequest instantiates a new ScaleRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewScaleRequestWithDefaults

`func NewScaleRequestWithDefaults() *ScaleRequest`

NewScaleRequestWithDefaults instantiates a new ScaleRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetReplicas

`func (o *ScaleRequest) GetReplicas() int32`

GetReplicas returns the Replicas field if non-nil, zero value otherwise.

### GetReplicasOk

`func (o *ScaleRequest) GetReplicasOk() (*int32, bool)`

GetReplicasOk returns a tuple with the Replicas field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReplicas

`func (o *ScaleRequest) SetReplicas(v int32)`

SetReplicas sets Replicas field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


