# StoredPermission

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CreatedAt** | **string** | When the permission was created. | 
**Id** | **string** | UUID identifier of this permission grant. | 
**Level** | [**PermissionLevel**](PermissionLevel.md) | The granted access level. | 
**ResourceId** | Pointer to **NullableString** | A specific resource id, or &#x60;None&#x60; for a wildcard (all resources of that kind). | [optional] 
**ResourceKind** | **string** | The kind of resource (e.g. &#x60;\&quot;deployment\&quot;&#x60;, &#x60;\&quot;project\&quot;&#x60;, &#x60;\&quot;secret\&quot;&#x60;). | 
**SubjectId** | **string** | The user or group id. | 
**SubjectKind** | [**SubjectKind**](SubjectKind.md) | Whether the subject is a user or a group. | 

## Methods

### NewStoredPermission

`func NewStoredPermission(createdAt string, id string, level PermissionLevel, resourceKind string, subjectId string, subjectKind SubjectKind, ) *StoredPermission`

NewStoredPermission instantiates a new StoredPermission object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewStoredPermissionWithDefaults

`func NewStoredPermissionWithDefaults() *StoredPermission`

NewStoredPermissionWithDefaults instantiates a new StoredPermission object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCreatedAt

`func (o *StoredPermission) GetCreatedAt() string`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *StoredPermission) GetCreatedAtOk() (*string, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *StoredPermission) SetCreatedAt(v string)`

SetCreatedAt sets CreatedAt field to given value.


### GetId

`func (o *StoredPermission) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *StoredPermission) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *StoredPermission) SetId(v string)`

SetId sets Id field to given value.


### GetLevel

`func (o *StoredPermission) GetLevel() PermissionLevel`

GetLevel returns the Level field if non-nil, zero value otherwise.

### GetLevelOk

`func (o *StoredPermission) GetLevelOk() (*PermissionLevel, bool)`

GetLevelOk returns a tuple with the Level field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLevel

`func (o *StoredPermission) SetLevel(v PermissionLevel)`

SetLevel sets Level field to given value.


### GetResourceId

`func (o *StoredPermission) GetResourceId() string`

GetResourceId returns the ResourceId field if non-nil, zero value otherwise.

### GetResourceIdOk

`func (o *StoredPermission) GetResourceIdOk() (*string, bool)`

GetResourceIdOk returns a tuple with the ResourceId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResourceId

`func (o *StoredPermission) SetResourceId(v string)`

SetResourceId sets ResourceId field to given value.

### HasResourceId

`func (o *StoredPermission) HasResourceId() bool`

HasResourceId returns a boolean if a field has been set.

### SetResourceIdNil

`func (o *StoredPermission) SetResourceIdNil(b bool)`

 SetResourceIdNil sets the value for ResourceId to be an explicit nil

### UnsetResourceId
`func (o *StoredPermission) UnsetResourceId()`

UnsetResourceId ensures that no value is present for ResourceId, not even an explicit nil
### GetResourceKind

`func (o *StoredPermission) GetResourceKind() string`

GetResourceKind returns the ResourceKind field if non-nil, zero value otherwise.

### GetResourceKindOk

`func (o *StoredPermission) GetResourceKindOk() (*string, bool)`

GetResourceKindOk returns a tuple with the ResourceKind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResourceKind

`func (o *StoredPermission) SetResourceKind(v string)`

SetResourceKind sets ResourceKind field to given value.


### GetSubjectId

`func (o *StoredPermission) GetSubjectId() string`

GetSubjectId returns the SubjectId field if non-nil, zero value otherwise.

### GetSubjectIdOk

`func (o *StoredPermission) GetSubjectIdOk() (*string, bool)`

GetSubjectIdOk returns a tuple with the SubjectId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSubjectId

`func (o *StoredPermission) SetSubjectId(v string)`

SetSubjectId sets SubjectId field to given value.


### GetSubjectKind

`func (o *StoredPermission) GetSubjectKind() SubjectKind`

GetSubjectKind returns the SubjectKind field if non-nil, zero value otherwise.

### GetSubjectKindOk

`func (o *StoredPermission) GetSubjectKindOk() (*SubjectKind, bool)`

GetSubjectKindOk returns a tuple with the SubjectKind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSubjectKind

`func (o *StoredPermission) SetSubjectKind(v SubjectKind)`

SetSubjectKind sets SubjectKind field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


