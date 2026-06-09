# InternalScaleResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Message** | Pointer to **NullableString** | Optional message | [optional] 
**Replicas** | **int32** | New replica count | 
**RerouteToOs** | Pointer to **NullableString** | When set, this agent refused the scale because it cannot run the workload&#39;s OS (H-7 &#x60;RouteToPeer&#x60; policy). The value is the OCI-canonical OS string the workload requires (&#x60;linux&#x60; / &#x60;windows&#x60; / &#x60;darwin&#x60;). The scheduler catches this and re-dispatches to a cluster peer whose &#x60;NodeState.os&#x60; matches. | [optional] 
**Service** | **string** | Service name that was scaled | 
**Success** | **bool** | Whether the operation succeeded | 

## Methods

### NewInternalScaleResponse

`func NewInternalScaleResponse(replicas int32, service string, success bool, ) *InternalScaleResponse`

NewInternalScaleResponse instantiates a new InternalScaleResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewInternalScaleResponseWithDefaults

`func NewInternalScaleResponseWithDefaults() *InternalScaleResponse`

NewInternalScaleResponseWithDefaults instantiates a new InternalScaleResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetMessage

`func (o *InternalScaleResponse) GetMessage() string`

GetMessage returns the Message field if non-nil, zero value otherwise.

### GetMessageOk

`func (o *InternalScaleResponse) GetMessageOk() (*string, bool)`

GetMessageOk returns a tuple with the Message field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMessage

`func (o *InternalScaleResponse) SetMessage(v string)`

SetMessage sets Message field to given value.

### HasMessage

`func (o *InternalScaleResponse) HasMessage() bool`

HasMessage returns a boolean if a field has been set.

### SetMessageNil

`func (o *InternalScaleResponse) SetMessageNil(b bool)`

 SetMessageNil sets the value for Message to be an explicit nil

### UnsetMessage
`func (o *InternalScaleResponse) UnsetMessage()`

UnsetMessage ensures that no value is present for Message, not even an explicit nil
### GetReplicas

`func (o *InternalScaleResponse) GetReplicas() int32`

GetReplicas returns the Replicas field if non-nil, zero value otherwise.

### GetReplicasOk

`func (o *InternalScaleResponse) GetReplicasOk() (*int32, bool)`

GetReplicasOk returns a tuple with the Replicas field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReplicas

`func (o *InternalScaleResponse) SetReplicas(v int32)`

SetReplicas sets Replicas field to given value.


### GetRerouteToOs

`func (o *InternalScaleResponse) GetRerouteToOs() string`

GetRerouteToOs returns the RerouteToOs field if non-nil, zero value otherwise.

### GetRerouteToOsOk

`func (o *InternalScaleResponse) GetRerouteToOsOk() (*string, bool)`

GetRerouteToOsOk returns a tuple with the RerouteToOs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRerouteToOs

`func (o *InternalScaleResponse) SetRerouteToOs(v string)`

SetRerouteToOs sets RerouteToOs field to given value.

### HasRerouteToOs

`func (o *InternalScaleResponse) HasRerouteToOs() bool`

HasRerouteToOs returns a boolean if a field has been set.

### SetRerouteToOsNil

`func (o *InternalScaleResponse) SetRerouteToOsNil(b bool)`

 SetRerouteToOsNil sets the value for RerouteToOs to be an explicit nil

### UnsetRerouteToOs
`func (o *InternalScaleResponse) UnsetRerouteToOs()`

UnsetRerouteToOs ensures that no value is present for RerouteToOs, not even an explicit nil
### GetService

`func (o *InternalScaleResponse) GetService() string`

GetService returns the Service field if non-nil, zero value otherwise.

### GetServiceOk

`func (o *InternalScaleResponse) GetServiceOk() (*string, bool)`

GetServiceOk returns a tuple with the Service field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetService

`func (o *InternalScaleResponse) SetService(v string)`

SetService sets Service field to given value.


### GetSuccess

`func (o *InternalScaleResponse) GetSuccess() bool`

GetSuccess returns the Success field if non-nil, zero value otherwise.

### GetSuccessOk

`func (o *InternalScaleResponse) GetSuccessOk() (*bool, bool)`

GetSuccessOk returns a tuple with the Success field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSuccess

`func (o *InternalScaleResponse) SetSuccess(v bool)`

SetSuccess sets Success field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


