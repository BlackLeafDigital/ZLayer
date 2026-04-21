# RegistryAuth

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AuthType** | Pointer to [**RegistryAuthType**](RegistryAuthType.md) | Which authentication scheme to use against the registry. | [optional] 
**Password** | **string** | Password or bearer token. **Never** logged or returned on any response — consumed once and dropped. | 
**Username** | **string** | Username for the registry (for basic auth) or a placeholder identifier when &#x60;auth_type &#x3D;&#x3D; Token&#x60;. | 

## Methods

### NewRegistryAuth

`func NewRegistryAuth(password string, username string, ) *RegistryAuth`

NewRegistryAuth instantiates a new RegistryAuth object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewRegistryAuthWithDefaults

`func NewRegistryAuthWithDefaults() *RegistryAuth`

NewRegistryAuthWithDefaults instantiates a new RegistryAuth object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAuthType

`func (o *RegistryAuth) GetAuthType() RegistryAuthType`

GetAuthType returns the AuthType field if non-nil, zero value otherwise.

### GetAuthTypeOk

`func (o *RegistryAuth) GetAuthTypeOk() (*RegistryAuthType, bool)`

GetAuthTypeOk returns a tuple with the AuthType field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAuthType

`func (o *RegistryAuth) SetAuthType(v RegistryAuthType)`

SetAuthType sets AuthType field to given value.

### HasAuthType

`func (o *RegistryAuth) HasAuthType() bool`

HasAuthType returns a boolean if a field has been set.

### GetPassword

`func (o *RegistryAuth) GetPassword() string`

GetPassword returns the Password field if non-nil, zero value otherwise.

### GetPasswordOk

`func (o *RegistryAuth) GetPasswordOk() (*string, bool)`

GetPasswordOk returns a tuple with the Password field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPassword

`func (o *RegistryAuth) SetPassword(v string)`

SetPassword sets Password field to given value.


### GetUsername

`func (o *RegistryAuth) GetUsername() string`

GetUsername returns the Username field if non-nil, zero value otherwise.

### GetUsernameOk

`func (o *RegistryAuth) GetUsernameOk() (*string, bool)`

GetUsernameOk returns a tuple with the Username field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUsername

`func (o *RegistryAuth) SetUsername(v string)`

SetUsername sets Username field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


