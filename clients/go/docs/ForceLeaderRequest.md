# ForceLeaderRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Confirm** | **string** | Confirmation string -- must be &#x60;\&quot;CONFIRM_FORCE_LEADER\&quot;&#x60; to prevent accidents | 

## Methods

### NewForceLeaderRequest

`func NewForceLeaderRequest(confirm string, ) *ForceLeaderRequest`

NewForceLeaderRequest instantiates a new ForceLeaderRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewForceLeaderRequestWithDefaults

`func NewForceLeaderRequestWithDefaults() *ForceLeaderRequest`

NewForceLeaderRequestWithDefaults instantiates a new ForceLeaderRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetConfirm

`func (o *ForceLeaderRequest) GetConfirm() string`

GetConfirm returns the Confirm field if non-nil, zero value otherwise.

### GetConfirmOk

`func (o *ForceLeaderRequest) GetConfirmOk() (*string, bool)`

GetConfirmOk returns a tuple with the Confirm field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetConfirm

`func (o *ForceLeaderRequest) SetConfirm(v string)`

SetConfirm sets Confirm field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


