# TestNotifierResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Message** | **string** | Status message. | 
**Success** | **bool** | Whether the test notification was sent successfully. | 

## Methods

### NewTestNotifierResponse

`func NewTestNotifierResponse(message string, success bool, ) *TestNotifierResponse`

NewTestNotifierResponse instantiates a new TestNotifierResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewTestNotifierResponseWithDefaults

`func NewTestNotifierResponseWithDefaults() *TestNotifierResponse`

NewTestNotifierResponseWithDefaults instantiates a new TestNotifierResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetMessage

`func (o *TestNotifierResponse) GetMessage() string`

GetMessage returns the Message field if non-nil, zero value otherwise.

### GetMessageOk

`func (o *TestNotifierResponse) GetMessageOk() (*string, bool)`

GetMessageOk returns a tuple with the Message field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMessage

`func (o *TestNotifierResponse) SetMessage(v string)`

SetMessage sets Message field to given value.


### GetSuccess

`func (o *TestNotifierResponse) GetSuccess() bool`

GetSuccess returns the Success field if non-nil, zero value otherwise.

### GetSuccessOk

`func (o *TestNotifierResponse) GetSuccessOk() (*bool, bool)`

GetSuccessOk returns a tuple with the Success field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSuccess

`func (o *TestNotifierResponse) SetSuccess(v bool)`

SetSuccess sets Success field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


