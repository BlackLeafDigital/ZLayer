# ContainerRestartPolicy

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Delay** | Pointer to **NullableString** | Humantime-formatted delay between restarts (e.g. &#x60;\&quot;500ms\&quot;&#x60;, &#x60;\&quot;2s\&quot;&#x60;). Accepted for forward-compatibility but currently ignored by the Docker backend: bollard&#39;s &#x60;RestartPolicy&#x60; has no per-kind delay field. When set, the runtime emits a warning. | [optional] 
**Kind** | [**ContainerRestartKind**](ContainerRestartKind.md) | Which restart policy to apply. | 
**MaxAttempts** | Pointer to **NullableInt32** | For &#x60;on_failure&#x60; only: maximum number of restart attempts before giving up. Ignored by other kinds. &#x60;None&#x60; means \&quot;retry forever\&quot;. | [optional] 

## Methods

### NewContainerRestartPolicy

`func NewContainerRestartPolicy(kind ContainerRestartKind, ) *ContainerRestartPolicy`

NewContainerRestartPolicy instantiates a new ContainerRestartPolicy object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerRestartPolicyWithDefaults

`func NewContainerRestartPolicyWithDefaults() *ContainerRestartPolicy`

NewContainerRestartPolicyWithDefaults instantiates a new ContainerRestartPolicy object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetDelay

`func (o *ContainerRestartPolicy) GetDelay() string`

GetDelay returns the Delay field if non-nil, zero value otherwise.

### GetDelayOk

`func (o *ContainerRestartPolicy) GetDelayOk() (*string, bool)`

GetDelayOk returns a tuple with the Delay field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDelay

`func (o *ContainerRestartPolicy) SetDelay(v string)`

SetDelay sets Delay field to given value.

### HasDelay

`func (o *ContainerRestartPolicy) HasDelay() bool`

HasDelay returns a boolean if a field has been set.

### SetDelayNil

`func (o *ContainerRestartPolicy) SetDelayNil(b bool)`

 SetDelayNil sets the value for Delay to be an explicit nil

### UnsetDelay
`func (o *ContainerRestartPolicy) UnsetDelay()`

UnsetDelay ensures that no value is present for Delay, not even an explicit nil
### GetKind

`func (o *ContainerRestartPolicy) GetKind() ContainerRestartKind`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *ContainerRestartPolicy) GetKindOk() (*ContainerRestartKind, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *ContainerRestartPolicy) SetKind(v ContainerRestartKind)`

SetKind sets Kind field to given value.


### GetMaxAttempts

`func (o *ContainerRestartPolicy) GetMaxAttempts() int32`

GetMaxAttempts returns the MaxAttempts field if non-nil, zero value otherwise.

### GetMaxAttemptsOk

`func (o *ContainerRestartPolicy) GetMaxAttemptsOk() (*int32, bool)`

GetMaxAttemptsOk returns a tuple with the MaxAttempts field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMaxAttempts

`func (o *ContainerRestartPolicy) SetMaxAttempts(v int32)`

SetMaxAttempts sets MaxAttempts field to given value.

### HasMaxAttempts

`func (o *ContainerRestartPolicy) HasMaxAttempts() bool`

HasMaxAttempts returns a boolean if a field has been set.

### SetMaxAttemptsNil

`func (o *ContainerRestartPolicy) SetMaxAttemptsNil(b bool)`

 SetMaxAttemptsNil sets the value for MaxAttempts to be an explicit nil

### UnsetMaxAttempts
`func (o *ContainerRestartPolicy) UnsetMaxAttempts()`

UnsetMaxAttempts ensures that no value is present for MaxAttempts, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


