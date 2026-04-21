# GrantPermissionRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Level** | [**PermissionLevel**](PermissionLevel.md) | The access level to grant. | 
**ResourceId** | Pointer to **NullableString** | A specific resource id, or omit for a wildcard (all resources of that kind). | [optional] 
**ResourceKind** | **string** | The kind of resource (e.g. &#x60;\&quot;deployment\&quot;&#x60;, &#x60;\&quot;project\&quot;&#x60;, &#x60;\&quot;secret\&quot;&#x60;). | 
**SubjectId** | **string** | The user or group id. | 
**SubjectKind** | [**SubjectKind**](SubjectKind.md) | Whether the subject is a user or a group. | 

## Methods

### NewGrantPermissionRequest

`func NewGrantPermissionRequest(level PermissionLevel, resourceKind string, subjectId string, subjectKind SubjectKind, ) *GrantPermissionRequest`

NewGrantPermissionRequest instantiates a new GrantPermissionRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewGrantPermissionRequestWithDefaults

`func NewGrantPermissionRequestWithDefaults() *GrantPermissionRequest`

NewGrantPermissionRequestWithDefaults instantiates a new GrantPermissionRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetLevel

`func (o *GrantPermissionRequest) GetLevel() PermissionLevel`

GetLevel returns the Level field if non-nil, zero value otherwise.

### GetLevelOk

`func (o *GrantPermissionRequest) GetLevelOk() (*PermissionLevel, bool)`

GetLevelOk returns a tuple with the Level field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLevel

`func (o *GrantPermissionRequest) SetLevel(v PermissionLevel)`

SetLevel sets Level field to given value.


### GetResourceId

`func (o *GrantPermissionRequest) GetResourceId() string`

GetResourceId returns the ResourceId field if non-nil, zero value otherwise.

### GetResourceIdOk

`func (o *GrantPermissionRequest) GetResourceIdOk() (*string, bool)`

GetResourceIdOk returns a tuple with the ResourceId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResourceId

`func (o *GrantPermissionRequest) SetResourceId(v string)`

SetResourceId sets ResourceId field to given value.

### HasResourceId

`func (o *GrantPermissionRequest) HasResourceId() bool`

HasResourceId returns a boolean if a field has been set.

### SetResourceIdNil

`func (o *GrantPermissionRequest) SetResourceIdNil(b bool)`

 SetResourceIdNil sets the value for ResourceId to be an explicit nil

### UnsetResourceId
`func (o *GrantPermissionRequest) UnsetResourceId()`

UnsetResourceId ensures that no value is present for ResourceId, not even an explicit nil
### GetResourceKind

`func (o *GrantPermissionRequest) GetResourceKind() string`

GetResourceKind returns the ResourceKind field if non-nil, zero value otherwise.

### GetResourceKindOk

`func (o *GrantPermissionRequest) GetResourceKindOk() (*string, bool)`

GetResourceKindOk returns a tuple with the ResourceKind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResourceKind

`func (o *GrantPermissionRequest) SetResourceKind(v string)`

SetResourceKind sets ResourceKind field to given value.


### GetSubjectId

`func (o *GrantPermissionRequest) GetSubjectId() string`

GetSubjectId returns the SubjectId field if non-nil, zero value otherwise.

### GetSubjectIdOk

`func (o *GrantPermissionRequest) GetSubjectIdOk() (*string, bool)`

GetSubjectIdOk returns a tuple with the SubjectId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSubjectId

`func (o *GrantPermissionRequest) SetSubjectId(v string)`

SetSubjectId sets SubjectId field to given value.


### GetSubjectKind

`func (o *GrantPermissionRequest) GetSubjectKind() SubjectKind`

GetSubjectKind returns the SubjectKind field if non-nil, zero value otherwise.

### GetSubjectKindOk

`func (o *GrantPermissionRequest) GetSubjectKindOk() (*SubjectKind, bool)`

GetSubjectKindOk returns a tuple with the SubjectKind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSubjectKind

`func (o *GrantPermissionRequest) SetSubjectKind(v SubjectKind)`

SetSubjectKind sets SubjectKind field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


