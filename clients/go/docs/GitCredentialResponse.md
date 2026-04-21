# GitCredentialResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Id** | **string** | Unique identifier. | 
**Kind** | [**GitCredentialKindSchema**](GitCredentialKindSchema.md) | Credential kind. | 
**Name** | **string** | Human-readable display label. | 

## Methods

### NewGitCredentialResponse

`func NewGitCredentialResponse(id string, kind GitCredentialKindSchema, name string, ) *GitCredentialResponse`

NewGitCredentialResponse instantiates a new GitCredentialResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewGitCredentialResponseWithDefaults

`func NewGitCredentialResponseWithDefaults() *GitCredentialResponse`

NewGitCredentialResponseWithDefaults instantiates a new GitCredentialResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetId

`func (o *GitCredentialResponse) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *GitCredentialResponse) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *GitCredentialResponse) SetId(v string)`

SetId sets Id field to given value.


### GetKind

`func (o *GitCredentialResponse) GetKind() GitCredentialKindSchema`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *GitCredentialResponse) GetKindOk() (*GitCredentialKindSchema, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *GitCredentialResponse) SetKind(v GitCredentialKindSchema)`

SetKind sets Kind field to given value.


### GetName

`func (o *GitCredentialResponse) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *GitCredentialResponse) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *GitCredentialResponse) SetName(v string)`

SetName sets Name field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


