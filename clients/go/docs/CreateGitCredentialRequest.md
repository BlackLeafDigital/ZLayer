# CreateGitCredentialRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Kind** | [**GitCredentialKindSchema**](GitCredentialKindSchema.md) | Credential kind. | 
**Name** | **string** | Human-readable label (e.g. &#x60;\&quot;GitHub PAT for ci\&quot;&#x60;). | 
**Value** | **string** | PAT or SSH key content (stored encrypted, never returned). | 

## Methods

### NewCreateGitCredentialRequest

`func NewCreateGitCredentialRequest(kind GitCredentialKindSchema, name string, value string, ) *CreateGitCredentialRequest`

NewCreateGitCredentialRequest instantiates a new CreateGitCredentialRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateGitCredentialRequestWithDefaults

`func NewCreateGitCredentialRequestWithDefaults() *CreateGitCredentialRequest`

NewCreateGitCredentialRequestWithDefaults instantiates a new CreateGitCredentialRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetKind

`func (o *CreateGitCredentialRequest) GetKind() GitCredentialKindSchema`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *CreateGitCredentialRequest) GetKindOk() (*GitCredentialKindSchema, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *CreateGitCredentialRequest) SetKind(v GitCredentialKindSchema)`

SetKind sets Kind field to given value.


### GetName

`func (o *CreateGitCredentialRequest) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateGitCredentialRequest) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateGitCredentialRequest) SetName(v string)`

SetName sets Name field to given value.


### GetValue

`func (o *CreateGitCredentialRequest) GetValue() string`

GetValue returns the Value field if non-nil, zero value otherwise.

### GetValueOk

`func (o *CreateGitCredentialRequest) GetValueOk() (*string, bool)`

GetValueOk returns a tuple with the Value field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetValue

`func (o *CreateGitCredentialRequest) SetValue(v string)`

SetValue sets Value field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


