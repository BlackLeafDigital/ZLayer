# TriggerBuildResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**BuildId** | **string** | Unique build ID for tracking | 
**Message** | **string** | Human-readable message | 

## Methods

### NewTriggerBuildResponse

`func NewTriggerBuildResponse(buildId string, message string, ) *TriggerBuildResponse`

NewTriggerBuildResponse instantiates a new TriggerBuildResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewTriggerBuildResponseWithDefaults

`func NewTriggerBuildResponseWithDefaults() *TriggerBuildResponse`

NewTriggerBuildResponseWithDefaults instantiates a new TriggerBuildResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetBuildId

`func (o *TriggerBuildResponse) GetBuildId() string`

GetBuildId returns the BuildId field if non-nil, zero value otherwise.

### GetBuildIdOk

`func (o *TriggerBuildResponse) GetBuildIdOk() (*string, bool)`

GetBuildIdOk returns a tuple with the BuildId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBuildId

`func (o *TriggerBuildResponse) SetBuildId(v string)`

SetBuildId sets BuildId field to given value.


### GetMessage

`func (o *TriggerBuildResponse) GetMessage() string`

GetMessage returns the Message field if non-nil, zero value otherwise.

### GetMessageOk

`func (o *TriggerBuildResponse) GetMessageOk() (*string, bool)`

GetMessageOk returns a tuple with the Message field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMessage

`func (o *TriggerBuildResponse) SetMessage(v string)`

SetMessage sets Message field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


