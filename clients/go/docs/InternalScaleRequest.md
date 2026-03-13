# InternalScaleRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Replicas** | **int32** | Target replica count | 
**Service** | **string** | Service name to scale | 

## Methods

### NewInternalScaleRequest

`func NewInternalScaleRequest(replicas int32, service string, ) *InternalScaleRequest`

NewInternalScaleRequest instantiates a new InternalScaleRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewInternalScaleRequestWithDefaults

`func NewInternalScaleRequestWithDefaults() *InternalScaleRequest`

NewInternalScaleRequestWithDefaults instantiates a new InternalScaleRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

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


