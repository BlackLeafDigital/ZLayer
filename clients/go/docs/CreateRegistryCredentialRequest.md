# CreateRegistryCredentialRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AuthType** | [**RegistryAuthTypeSchema**](RegistryAuthTypeSchema.md) | Authentication method. | 
**Password** | **string** | Password or token value (stored encrypted, never returned). | 
**Registry** | **string** | Registry hostname (e.g. &#x60;\&quot;docker.io\&quot;&#x60;). | 
**Username** | **string** | Username for authentication. | 

## Methods

### NewCreateRegistryCredentialRequest

`func NewCreateRegistryCredentialRequest(authType RegistryAuthTypeSchema, password string, registry string, username string, ) *CreateRegistryCredentialRequest`

NewCreateRegistryCredentialRequest instantiates a new CreateRegistryCredentialRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateRegistryCredentialRequestWithDefaults

`func NewCreateRegistryCredentialRequestWithDefaults() *CreateRegistryCredentialRequest`

NewCreateRegistryCredentialRequestWithDefaults instantiates a new CreateRegistryCredentialRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAuthType

`func (o *CreateRegistryCredentialRequest) GetAuthType() RegistryAuthTypeSchema`

GetAuthType returns the AuthType field if non-nil, zero value otherwise.

### GetAuthTypeOk

`func (o *CreateRegistryCredentialRequest) GetAuthTypeOk() (*RegistryAuthTypeSchema, bool)`

GetAuthTypeOk returns a tuple with the AuthType field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAuthType

`func (o *CreateRegistryCredentialRequest) SetAuthType(v RegistryAuthTypeSchema)`

SetAuthType sets AuthType field to given value.


### GetPassword

`func (o *CreateRegistryCredentialRequest) GetPassword() string`

GetPassword returns the Password field if non-nil, zero value otherwise.

### GetPasswordOk

`func (o *CreateRegistryCredentialRequest) GetPasswordOk() (*string, bool)`

GetPasswordOk returns a tuple with the Password field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPassword

`func (o *CreateRegistryCredentialRequest) SetPassword(v string)`

SetPassword sets Password field to given value.


### GetRegistry

`func (o *CreateRegistryCredentialRequest) GetRegistry() string`

GetRegistry returns the Registry field if non-nil, zero value otherwise.

### GetRegistryOk

`func (o *CreateRegistryCredentialRequest) GetRegistryOk() (*string, bool)`

GetRegistryOk returns a tuple with the Registry field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRegistry

`func (o *CreateRegistryCredentialRequest) SetRegistry(v string)`

SetRegistry sets Registry field to given value.


### GetUsername

`func (o *CreateRegistryCredentialRequest) GetUsername() string`

GetUsername returns the Username field if non-nil, zero value otherwise.

### GetUsernameOk

`func (o *CreateRegistryCredentialRequest) GetUsernameOk() (*string, bool)`

GetUsernameOk returns a tuple with the Username field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUsername

`func (o *CreateRegistryCredentialRequest) SetUsername(v string)`

SetUsername sets Username field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


