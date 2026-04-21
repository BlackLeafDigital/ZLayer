# ContainerHealthInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**FailingStreak** | Pointer to **NullableInt32** | Consecutive failing probe count, when the runtime tracks it. | [optional] 
**LastOutput** | Pointer to **NullableString** | Output from the most recent failing probe, when available. | [optional] 
**Status** | **string** | One of &#x60;\&quot;none\&quot;&#x60;, &#x60;\&quot;starting\&quot;&#x60;, &#x60;\&quot;healthy\&quot;&#x60;, &#x60;\&quot;unhealthy\&quot;&#x60; (Docker &#x60;HealthStatusEnum&#x60;). Empty / missing upstream values normalise to &#x60;\&quot;none\&quot;&#x60;. | 

## Methods

### NewContainerHealthInfo

`func NewContainerHealthInfo(status string, ) *ContainerHealthInfo`

NewContainerHealthInfo instantiates a new ContainerHealthInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerHealthInfoWithDefaults

`func NewContainerHealthInfoWithDefaults() *ContainerHealthInfo`

NewContainerHealthInfoWithDefaults instantiates a new ContainerHealthInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetFailingStreak

`func (o *ContainerHealthInfo) GetFailingStreak() int32`

GetFailingStreak returns the FailingStreak field if non-nil, zero value otherwise.

### GetFailingStreakOk

`func (o *ContainerHealthInfo) GetFailingStreakOk() (*int32, bool)`

GetFailingStreakOk returns a tuple with the FailingStreak field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetFailingStreak

`func (o *ContainerHealthInfo) SetFailingStreak(v int32)`

SetFailingStreak sets FailingStreak field to given value.

### HasFailingStreak

`func (o *ContainerHealthInfo) HasFailingStreak() bool`

HasFailingStreak returns a boolean if a field has been set.

### SetFailingStreakNil

`func (o *ContainerHealthInfo) SetFailingStreakNil(b bool)`

 SetFailingStreakNil sets the value for FailingStreak to be an explicit nil

### UnsetFailingStreak
`func (o *ContainerHealthInfo) UnsetFailingStreak()`

UnsetFailingStreak ensures that no value is present for FailingStreak, not even an explicit nil
### GetLastOutput

`func (o *ContainerHealthInfo) GetLastOutput() string`

GetLastOutput returns the LastOutput field if non-nil, zero value otherwise.

### GetLastOutputOk

`func (o *ContainerHealthInfo) GetLastOutputOk() (*string, bool)`

GetLastOutputOk returns a tuple with the LastOutput field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastOutput

`func (o *ContainerHealthInfo) SetLastOutput(v string)`

SetLastOutput sets LastOutput field to given value.

### HasLastOutput

`func (o *ContainerHealthInfo) HasLastOutput() bool`

HasLastOutput returns a boolean if a field has been set.

### SetLastOutputNil

`func (o *ContainerHealthInfo) SetLastOutputNil(b bool)`

 SetLastOutputNil sets the value for LastOutput to be an explicit nil

### UnsetLastOutput
`func (o *ContainerHealthInfo) UnsetLastOutput()`

UnsetLastOutput ensures that no value is present for LastOutput, not even an explicit nil
### GetStatus

`func (o *ContainerHealthInfo) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *ContainerHealthInfo) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *ContainerHealthInfo) SetStatus(v string)`

SetStatus sets Status field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


