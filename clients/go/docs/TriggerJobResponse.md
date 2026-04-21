# TriggerJobResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**ExecutionId** | **string** | Unique execution ID for tracking | 
**Message** | **string** | Human-readable message | 

## Methods

### NewTriggerJobResponse

`func NewTriggerJobResponse(executionId string, message string, ) *TriggerJobResponse`

NewTriggerJobResponse instantiates a new TriggerJobResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewTriggerJobResponseWithDefaults

`func NewTriggerJobResponseWithDefaults() *TriggerJobResponse`

NewTriggerJobResponseWithDefaults instantiates a new TriggerJobResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetExecutionId

`func (o *TriggerJobResponse) GetExecutionId() string`

GetExecutionId returns the ExecutionId field if non-nil, zero value otherwise.

### GetExecutionIdOk

`func (o *TriggerJobResponse) GetExecutionIdOk() (*string, bool)`

GetExecutionIdOk returns a tuple with the ExecutionId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExecutionId

`func (o *TriggerJobResponse) SetExecutionId(v string)`

SetExecutionId sets ExecutionId field to given value.


### GetMessage

`func (o *TriggerJobResponse) GetMessage() string`

GetMessage returns the Message field if non-nil, zero value otherwise.

### GetMessageOk

`func (o *TriggerJobResponse) GetMessageOk() (*string, bool)`

GetMessageOk returns a tuple with the Message field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMessage

`func (o *TriggerJobResponse) SetMessage(v string)`

SetMessage sets Message field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


