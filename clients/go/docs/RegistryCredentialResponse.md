# RegistryCredentialResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AuthType** | [**RegistryAuthTypeSchema**](RegistryAuthTypeSchema.md) | Authentication method. | 
**Id** | **string** | Unique identifier. | 
**Registry** | **string** | Registry hostname, e.g. &#x60;\&quot;docker.io\&quot;&#x60;, &#x60;\&quot;ghcr.io\&quot;&#x60;. | 
**Username** | **string** | Username for authentication. | 

## Methods

### NewRegistryCredentialResponse

`func NewRegistryCredentialResponse(authType RegistryAuthTypeSchema, id string, registry string, username string, ) *RegistryCredentialResponse`

NewRegistryCredentialResponse instantiates a new RegistryCredentialResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewRegistryCredentialResponseWithDefaults

`func NewRegistryCredentialResponseWithDefaults() *RegistryCredentialResponse`

NewRegistryCredentialResponseWithDefaults instantiates a new RegistryCredentialResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAuthType

`func (o *RegistryCredentialResponse) GetAuthType() RegistryAuthTypeSchema`

GetAuthType returns the AuthType field if non-nil, zero value otherwise.

### GetAuthTypeOk

`func (o *RegistryCredentialResponse) GetAuthTypeOk() (*RegistryAuthTypeSchema, bool)`

GetAuthTypeOk returns a tuple with the AuthType field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAuthType

`func (o *RegistryCredentialResponse) SetAuthType(v RegistryAuthTypeSchema)`

SetAuthType sets AuthType field to given value.


### GetId

`func (o *RegistryCredentialResponse) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *RegistryCredentialResponse) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *RegistryCredentialResponse) SetId(v string)`

SetId sets Id field to given value.


### GetRegistry

`func (o *RegistryCredentialResponse) GetRegistry() string`

GetRegistry returns the Registry field if non-nil, zero value otherwise.

### GetRegistryOk

`func (o *RegistryCredentialResponse) GetRegistryOk() (*string, bool)`

GetRegistryOk returns a tuple with the Registry field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRegistry

`func (o *RegistryCredentialResponse) SetRegistry(v string)`

SetRegistry sets Registry field to given value.


### GetUsername

`func (o *RegistryCredentialResponse) GetUsername() string`

GetUsername returns the Username field if non-nil, zero value otherwise.

### GetUsernameOk

`func (o *RegistryCredentialResponse) GetUsernameOk() (*string, bool)`

GetUsernameOk returns a tuple with the Username field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUsername

`func (o *RegistryCredentialResponse) SetUsername(v string)`

SetUsername sets Username field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


