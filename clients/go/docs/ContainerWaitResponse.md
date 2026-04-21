# ContainerWaitResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**ExitCode** | **int32** | Exit code (0 &#x3D; success). When the container was killed by signal &#x60;N&#x60;, this is typically &#x60;128 + N&#x60;. | 
**FinishedAt** | Pointer to **NullableString** | RFC3339 timestamp of when the container exited, if reported by the runtime. | [optional] 
**Id** | **string** | Container identifier | 
**Reason** | Pointer to **NullableString** | Classification of the exit. One of &#x60;\&quot;exited\&quot;&#x60;, &#x60;\&quot;signal\&quot;&#x60;, &#x60;\&quot;oom_killed\&quot;&#x60;, or &#x60;\&quot;runtime_error\&quot;&#x60;. Absent when the runtime didn&#39;t classify the exit. | [optional] 
**Signal** | Pointer to **NullableString** | Signal name when &#x60;reason &#x3D;&#x3D; \&quot;signal\&quot;&#x60;, e.g. &#x60;\&quot;SIGKILL\&quot;&#x60;. Absent when the runtime couldn&#39;t determine it (or the exit wasn&#39;t a signal death). | [optional] 

## Methods

### NewContainerWaitResponse

`func NewContainerWaitResponse(exitCode int32, id string, ) *ContainerWaitResponse`

NewContainerWaitResponse instantiates a new ContainerWaitResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerWaitResponseWithDefaults

`func NewContainerWaitResponseWithDefaults() *ContainerWaitResponse`

NewContainerWaitResponseWithDefaults instantiates a new ContainerWaitResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetExitCode

`func (o *ContainerWaitResponse) GetExitCode() int32`

GetExitCode returns the ExitCode field if non-nil, zero value otherwise.

### GetExitCodeOk

`func (o *ContainerWaitResponse) GetExitCodeOk() (*int32, bool)`

GetExitCodeOk returns a tuple with the ExitCode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExitCode

`func (o *ContainerWaitResponse) SetExitCode(v int32)`

SetExitCode sets ExitCode field to given value.


### GetFinishedAt

`func (o *ContainerWaitResponse) GetFinishedAt() string`

GetFinishedAt returns the FinishedAt field if non-nil, zero value otherwise.

### GetFinishedAtOk

`func (o *ContainerWaitResponse) GetFinishedAtOk() (*string, bool)`

GetFinishedAtOk returns a tuple with the FinishedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetFinishedAt

`func (o *ContainerWaitResponse) SetFinishedAt(v string)`

SetFinishedAt sets FinishedAt field to given value.

### HasFinishedAt

`func (o *ContainerWaitResponse) HasFinishedAt() bool`

HasFinishedAt returns a boolean if a field has been set.

### SetFinishedAtNil

`func (o *ContainerWaitResponse) SetFinishedAtNil(b bool)`

 SetFinishedAtNil sets the value for FinishedAt to be an explicit nil

### UnsetFinishedAt
`func (o *ContainerWaitResponse) UnsetFinishedAt()`

UnsetFinishedAt ensures that no value is present for FinishedAt, not even an explicit nil
### GetId

`func (o *ContainerWaitResponse) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *ContainerWaitResponse) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *ContainerWaitResponse) SetId(v string)`

SetId sets Id field to given value.


### GetReason

`func (o *ContainerWaitResponse) GetReason() string`

GetReason returns the Reason field if non-nil, zero value otherwise.

### GetReasonOk

`func (o *ContainerWaitResponse) GetReasonOk() (*string, bool)`

GetReasonOk returns a tuple with the Reason field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReason

`func (o *ContainerWaitResponse) SetReason(v string)`

SetReason sets Reason field to given value.

### HasReason

`func (o *ContainerWaitResponse) HasReason() bool`

HasReason returns a boolean if a field has been set.

### SetReasonNil

`func (o *ContainerWaitResponse) SetReasonNil(b bool)`

 SetReasonNil sets the value for Reason to be an explicit nil

### UnsetReason
`func (o *ContainerWaitResponse) UnsetReason()`

UnsetReason ensures that no value is present for Reason, not even an explicit nil
### GetSignal

`func (o *ContainerWaitResponse) GetSignal() string`

GetSignal returns the Signal field if non-nil, zero value otherwise.

### GetSignalOk

`func (o *ContainerWaitResponse) GetSignalOk() (*string, bool)`

GetSignalOk returns a tuple with the Signal field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSignal

`func (o *ContainerWaitResponse) SetSignal(v string)`

SetSignal sets Signal field to given value.

### HasSignal

`func (o *ContainerWaitResponse) HasSignal() bool`

HasSignal returns a boolean if a field has been set.

### SetSignalNil

`func (o *ContainerWaitResponse) SetSignalNil(b bool)`

 SetSignalNil sets the value for Signal to be an explicit nil

### UnsetSignal
`func (o *ContainerWaitResponse) UnsetSignal()`

UnsetSignal ensures that no value is present for Signal, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


