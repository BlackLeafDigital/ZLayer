# SecretMetadataResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CreatedAt** | **int64** | Unix timestamp when the secret was created. | 
**Name** | **string** | The name/identifier of the secret. | 
**UpdatedAt** | **int64** | Unix timestamp when the secret was last updated. | 
**Value** | Pointer to **NullableString** | Plaintext value — populated only on &#x60;?reveal&#x3D;true&#x60; admin reads. | [optional] 
**Version** | **int32** | Version number of the secret (incremented on each update). | 

## Methods

### NewSecretMetadataResponse

`func NewSecretMetadataResponse(createdAt int64, name string, updatedAt int64, version int32, ) *SecretMetadataResponse`

NewSecretMetadataResponse instantiates a new SecretMetadataResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewSecretMetadataResponseWithDefaults

`func NewSecretMetadataResponseWithDefaults() *SecretMetadataResponse`

NewSecretMetadataResponseWithDefaults instantiates a new SecretMetadataResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCreatedAt

`func (o *SecretMetadataResponse) GetCreatedAt() int64`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *SecretMetadataResponse) GetCreatedAtOk() (*int64, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *SecretMetadataResponse) SetCreatedAt(v int64)`

SetCreatedAt sets CreatedAt field to given value.


### GetName

`func (o *SecretMetadataResponse) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *SecretMetadataResponse) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *SecretMetadataResponse) SetName(v string)`

SetName sets Name field to given value.


### GetUpdatedAt

`func (o *SecretMetadataResponse) GetUpdatedAt() int64`

GetUpdatedAt returns the UpdatedAt field if non-nil, zero value otherwise.

### GetUpdatedAtOk

`func (o *SecretMetadataResponse) GetUpdatedAtOk() (*int64, bool)`

GetUpdatedAtOk returns a tuple with the UpdatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUpdatedAt

`func (o *SecretMetadataResponse) SetUpdatedAt(v int64)`

SetUpdatedAt sets UpdatedAt field to given value.


### GetValue

`func (o *SecretMetadataResponse) GetValue() string`

GetValue returns the Value field if non-nil, zero value otherwise.

### GetValueOk

`func (o *SecretMetadataResponse) GetValueOk() (*string, bool)`

GetValueOk returns a tuple with the Value field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetValue

`func (o *SecretMetadataResponse) SetValue(v string)`

SetValue sets Value field to given value.

### HasValue

`func (o *SecretMetadataResponse) HasValue() bool`

HasValue returns a boolean if a field has been set.

### SetValueNil

`func (o *SecretMetadataResponse) SetValueNil(b bool)`

 SetValueNil sets the value for Value to be an explicit nil

### UnsetValue
`func (o *SecretMetadataResponse) UnsetValue()`

UnsetValue ensures that no value is present for Value, not even an explicit nil
### GetVersion

`func (o *SecretMetadataResponse) GetVersion() int32`

GetVersion returns the Version field if non-nil, zero value otherwise.

### GetVersionOk

`func (o *SecretMetadataResponse) GetVersionOk() (*int32, bool)`

GetVersionOk returns a tuple with the Version field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetVersion

`func (o *SecretMetadataResponse) SetVersion(v int32)`

SetVersion sets Version field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


