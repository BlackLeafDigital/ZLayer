# PullImageRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**PullPolicy** | Pointer to **NullableString** | Pull policy override. Accepts &#x60;\&quot;always\&quot;&#x60;, &#x60;\&quot;if_not_present\&quot;&#x60;, or &#x60;\&quot;never\&quot;&#x60;. Defaults to &#x60;\&quot;always\&quot;&#x60; when omitted. | [optional] 
**Reference** | **string** | OCI image reference to pull, e.g. &#x60;docker.io/library/nginx:latest&#x60;. | 
**RegistryAuth** | Pointer to [**NullableRegistryAuth**](RegistryAuth.md) | Inline Docker/OCI registry credentials used for this pull only. Not persisted, never logged, never echoed back on a response. Takes precedence over &#x60;registry_credential_id&#x60;. | [optional] 
**RegistryCredentialId** | Pointer to **NullableString** | Id of a persisted registry credential (from &#x60;POST /api/v1/credentials/registry&#x60;) to use for this pull. Ignored when [&#x60;Self::registry_auth&#x60;] is also supplied (inline auth wins). Requires the daemon to be configured with a credential store. | [optional] 

## Methods

### NewPullImageRequest

`func NewPullImageRequest(reference string, ) *PullImageRequest`

NewPullImageRequest instantiates a new PullImageRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewPullImageRequestWithDefaults

`func NewPullImageRequestWithDefaults() *PullImageRequest`

NewPullImageRequestWithDefaults instantiates a new PullImageRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetPullPolicy

`func (o *PullImageRequest) GetPullPolicy() string`

GetPullPolicy returns the PullPolicy field if non-nil, zero value otherwise.

### GetPullPolicyOk

`func (o *PullImageRequest) GetPullPolicyOk() (*string, bool)`

GetPullPolicyOk returns a tuple with the PullPolicy field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPullPolicy

`func (o *PullImageRequest) SetPullPolicy(v string)`

SetPullPolicy sets PullPolicy field to given value.

### HasPullPolicy

`func (o *PullImageRequest) HasPullPolicy() bool`

HasPullPolicy returns a boolean if a field has been set.

### SetPullPolicyNil

`func (o *PullImageRequest) SetPullPolicyNil(b bool)`

 SetPullPolicyNil sets the value for PullPolicy to be an explicit nil

### UnsetPullPolicy
`func (o *PullImageRequest) UnsetPullPolicy()`

UnsetPullPolicy ensures that no value is present for PullPolicy, not even an explicit nil
### GetReference

`func (o *PullImageRequest) GetReference() string`

GetReference returns the Reference field if non-nil, zero value otherwise.

### GetReferenceOk

`func (o *PullImageRequest) GetReferenceOk() (*string, bool)`

GetReferenceOk returns a tuple with the Reference field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReference

`func (o *PullImageRequest) SetReference(v string)`

SetReference sets Reference field to given value.


### GetRegistryAuth

`func (o *PullImageRequest) GetRegistryAuth() RegistryAuth`

GetRegistryAuth returns the RegistryAuth field if non-nil, zero value otherwise.

### GetRegistryAuthOk

`func (o *PullImageRequest) GetRegistryAuthOk() (*RegistryAuth, bool)`

GetRegistryAuthOk returns a tuple with the RegistryAuth field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRegistryAuth

`func (o *PullImageRequest) SetRegistryAuth(v RegistryAuth)`

SetRegistryAuth sets RegistryAuth field to given value.

### HasRegistryAuth

`func (o *PullImageRequest) HasRegistryAuth() bool`

HasRegistryAuth returns a boolean if a field has been set.

### SetRegistryAuthNil

`func (o *PullImageRequest) SetRegistryAuthNil(b bool)`

 SetRegistryAuthNil sets the value for RegistryAuth to be an explicit nil

### UnsetRegistryAuth
`func (o *PullImageRequest) UnsetRegistryAuth()`

UnsetRegistryAuth ensures that no value is present for RegistryAuth, not even an explicit nil
### GetRegistryCredentialId

`func (o *PullImageRequest) GetRegistryCredentialId() string`

GetRegistryCredentialId returns the RegistryCredentialId field if non-nil, zero value otherwise.

### GetRegistryCredentialIdOk

`func (o *PullImageRequest) GetRegistryCredentialIdOk() (*string, bool)`

GetRegistryCredentialIdOk returns a tuple with the RegistryCredentialId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRegistryCredentialId

`func (o *PullImageRequest) SetRegistryCredentialId(v string)`

SetRegistryCredentialId sets RegistryCredentialId field to given value.

### HasRegistryCredentialId

`func (o *PullImageRequest) HasRegistryCredentialId() bool`

HasRegistryCredentialId returns a boolean if a field has been set.

### SetRegistryCredentialIdNil

`func (o *PullImageRequest) SetRegistryCredentialIdNil(b bool)`

 SetRegistryCredentialIdNil sets the value for RegistryCredentialId to be an explicit nil

### UnsetRegistryCredentialId
`func (o *PullImageRequest) UnsetRegistryCredentialId()`

UnsetRegistryCredentialId ensures that no value is present for RegistryCredentialId, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


