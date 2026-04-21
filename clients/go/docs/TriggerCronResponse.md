# TriggerCronResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**ExecutionId** | **string** | Execution ID for tracking | 
**Message** | **string** | Human-readable message | 

## Methods

### NewTriggerCronResponse

`func NewTriggerCronResponse(executionId string, message string, ) *TriggerCronResponse`

NewTriggerCronResponse instantiates a new TriggerCronResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewTriggerCronResponseWithDefaults

`func NewTriggerCronResponseWithDefaults() *TriggerCronResponse`

NewTriggerCronResponseWithDefaults instantiates a new TriggerCronResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetExecutionId

`func (o *TriggerCronResponse) GetExecutionId() string`

GetExecutionId returns the ExecutionId field if non-nil, zero value otherwise.

### GetExecutionIdOk

`func (o *TriggerCronResponse) GetExecutionIdOk() (*string, bool)`

GetExecutionIdOk returns a tuple with the ExecutionId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExecutionId

`func (o *TriggerCronResponse) SetExecutionId(v string)`

SetExecutionId sets ExecutionId field to given value.


### GetMessage

`func (o *TriggerCronResponse) GetMessage() string`

GetMessage returns the Message field if non-nil, zero value otherwise.

### GetMessageOk

`func (o *TriggerCronResponse) GetMessageOk() (*string, bool)`

GetMessageOk returns a tuple with the Message field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMessage

`func (o *TriggerCronResponse) SetMessage(v string)`

SetMessage sets Message field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


