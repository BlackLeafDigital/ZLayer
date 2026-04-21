# RotateSecretRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Value** | **string** | The new secret value (will be encrypted at rest). | 

## Methods

### NewRotateSecretRequest

`func NewRotateSecretRequest(value string, ) *RotateSecretRequest`

NewRotateSecretRequest instantiates a new RotateSecretRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewRotateSecretRequestWithDefaults

`func NewRotateSecretRequestWithDefaults() *RotateSecretRequest`

NewRotateSecretRequestWithDefaults instantiates a new RotateSecretRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetValue

`func (o *RotateSecretRequest) GetValue() string`

GetValue returns the Value field if non-nil, zero value otherwise.

### GetValueOk

`func (o *RotateSecretRequest) GetValueOk() (*string, bool)`

GetValueOk returns a tuple with the Value field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetValue

`func (o *RotateSecretRequest) SetValue(v string)`

SetValue sets Value field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


