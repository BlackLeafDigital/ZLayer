# RotateSecretResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Name** | **string** | The secret name. | 
**NewVersion** | **int32** | Version after rotation. | 
**PreviousVersion** | Pointer to **NullableInt32** | Version prior to rotation. &#x60;None&#x60; if the secret did not exist (won&#39;t happen today — rotate rejects missing secrets — but preserved for forward compatibility). | [optional] 

## Methods

### NewRotateSecretResponse

`func NewRotateSecretResponse(name string, newVersion int32, ) *RotateSecretResponse`

NewRotateSecretResponse instantiates a new RotateSecretResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewRotateSecretResponseWithDefaults

`func NewRotateSecretResponseWithDefaults() *RotateSecretResponse`

NewRotateSecretResponseWithDefaults instantiates a new RotateSecretResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetName

`func (o *RotateSecretResponse) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *RotateSecretResponse) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *RotateSecretResponse) SetName(v string)`

SetName sets Name field to given value.


### GetNewVersion

`func (o *RotateSecretResponse) GetNewVersion() int32`

GetNewVersion returns the NewVersion field if non-nil, zero value otherwise.

### GetNewVersionOk

`func (o *RotateSecretResponse) GetNewVersionOk() (*int32, bool)`

GetNewVersionOk returns a tuple with the NewVersion field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNewVersion

`func (o *RotateSecretResponse) SetNewVersion(v int32)`

SetNewVersion sets NewVersion field to given value.


### GetPreviousVersion

`func (o *RotateSecretResponse) GetPreviousVersion() int32`

GetPreviousVersion returns the PreviousVersion field if non-nil, zero value otherwise.

### GetPreviousVersionOk

`func (o *RotateSecretResponse) GetPreviousVersionOk() (*int32, bool)`

GetPreviousVersionOk returns a tuple with the PreviousVersion field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPreviousVersion

`func (o *RotateSecretResponse) SetPreviousVersion(v int32)`

SetPreviousVersion sets PreviousVersion field to given value.

### HasPreviousVersion

`func (o *RotateSecretResponse) HasPreviousVersion() bool`

HasPreviousVersion returns a boolean if a field has been set.

### SetPreviousVersionNil

`func (o *RotateSecretResponse) SetPreviousVersionNil(b bool)`

 SetPreviousVersionNil sets the value for PreviousVersion to be an explicit nil

### UnsetPreviousVersion
`func (o *RotateSecretResponse) UnsetPreviousVersion()`

UnsetPreviousVersion ensures that no value is present for PreviousVersion, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


