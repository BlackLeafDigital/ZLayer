# KillContainerRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Signal** | Pointer to **NullableString** | Signal name to send (e.g. &#x60;\&quot;SIGTERM\&quot;&#x60;, &#x60;\&quot;SIGINT\&quot;&#x60;). Accepts both the &#x60;SIG&#x60;-prefixed and bare forms. When omitted, defaults to &#x60;SIGKILL&#x60;. | [optional] 

## Methods

### NewKillContainerRequest

`func NewKillContainerRequest() *KillContainerRequest`

NewKillContainerRequest instantiates a new KillContainerRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewKillContainerRequestWithDefaults

`func NewKillContainerRequestWithDefaults() *KillContainerRequest`

NewKillContainerRequestWithDefaults instantiates a new KillContainerRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetSignal

`func (o *KillContainerRequest) GetSignal() string`

GetSignal returns the Signal field if non-nil, zero value otherwise.

### GetSignalOk

`func (o *KillContainerRequest) GetSignalOk() (*string, bool)`

GetSignalOk returns a tuple with the Signal field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSignal

`func (o *KillContainerRequest) SetSignal(v string)`

SetSignal sets Signal field to given value.

### HasSignal

`func (o *KillContainerRequest) HasSignal() bool`

HasSignal returns a boolean if a field has been set.

### SetSignalNil

`func (o *KillContainerRequest) SetSignalNil(b bool)`

 SetSignalNil sets the value for Signal to be an explicit nil

### UnsetSignal
`func (o *KillContainerRequest) UnsetSignal()`

UnsetSignal ensures that no value is present for Signal, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


