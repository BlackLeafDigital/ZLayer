# BuildRequestWithContext

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**BuildArgs** | Pointer to **map[string]string** | Build arguments | [optional] 
**ContextPath** | **string** | Path to the build context on the server | 
**NoCache** | Pointer to **bool** | Disable cache | [optional] 
**Push** | Pointer to **bool** | Push after build | [optional] 
**Runtime** | Pointer to **NullableString** | Use runtime template instead of Dockerfile | [optional] 
**Tags** | Pointer to **[]string** | Tags to apply | [optional] 
**Target** | Pointer to **NullableString** | Target stage | [optional] 

## Methods

### NewBuildRequestWithContext

`func NewBuildRequestWithContext(contextPath string, ) *BuildRequestWithContext`

NewBuildRequestWithContext instantiates a new BuildRequestWithContext object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewBuildRequestWithContextWithDefaults

`func NewBuildRequestWithContextWithDefaults() *BuildRequestWithContext`

NewBuildRequestWithContextWithDefaults instantiates a new BuildRequestWithContext object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetBuildArgs

`func (o *BuildRequestWithContext) GetBuildArgs() map[string]string`

GetBuildArgs returns the BuildArgs field if non-nil, zero value otherwise.

### GetBuildArgsOk

`func (o *BuildRequestWithContext) GetBuildArgsOk() (*map[string]string, bool)`

GetBuildArgsOk returns a tuple with the BuildArgs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBuildArgs

`func (o *BuildRequestWithContext) SetBuildArgs(v map[string]string)`

SetBuildArgs sets BuildArgs field to given value.

### HasBuildArgs

`func (o *BuildRequestWithContext) HasBuildArgs() bool`

HasBuildArgs returns a boolean if a field has been set.

### GetContextPath

`func (o *BuildRequestWithContext) GetContextPath() string`

GetContextPath returns the ContextPath field if non-nil, zero value otherwise.

### GetContextPathOk

`func (o *BuildRequestWithContext) GetContextPathOk() (*string, bool)`

GetContextPathOk returns a tuple with the ContextPath field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetContextPath

`func (o *BuildRequestWithContext) SetContextPath(v string)`

SetContextPath sets ContextPath field to given value.


### GetNoCache

`func (o *BuildRequestWithContext) GetNoCache() bool`

GetNoCache returns the NoCache field if non-nil, zero value otherwise.

### GetNoCacheOk

`func (o *BuildRequestWithContext) GetNoCacheOk() (*bool, bool)`

GetNoCacheOk returns a tuple with the NoCache field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNoCache

`func (o *BuildRequestWithContext) SetNoCache(v bool)`

SetNoCache sets NoCache field to given value.

### HasNoCache

`func (o *BuildRequestWithContext) HasNoCache() bool`

HasNoCache returns a boolean if a field has been set.

### GetPush

`func (o *BuildRequestWithContext) GetPush() bool`

GetPush returns the Push field if non-nil, zero value otherwise.

### GetPushOk

`func (o *BuildRequestWithContext) GetPushOk() (*bool, bool)`

GetPushOk returns a tuple with the Push field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPush

`func (o *BuildRequestWithContext) SetPush(v bool)`

SetPush sets Push field to given value.

### HasPush

`func (o *BuildRequestWithContext) HasPush() bool`

HasPush returns a boolean if a field has been set.

### GetRuntime

`func (o *BuildRequestWithContext) GetRuntime() string`

GetRuntime returns the Runtime field if non-nil, zero value otherwise.

### GetRuntimeOk

`func (o *BuildRequestWithContext) GetRuntimeOk() (*string, bool)`

GetRuntimeOk returns a tuple with the Runtime field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRuntime

`func (o *BuildRequestWithContext) SetRuntime(v string)`

SetRuntime sets Runtime field to given value.

### HasRuntime

`func (o *BuildRequestWithContext) HasRuntime() bool`

HasRuntime returns a boolean if a field has been set.

### SetRuntimeNil

`func (o *BuildRequestWithContext) SetRuntimeNil(b bool)`

 SetRuntimeNil sets the value for Runtime to be an explicit nil

### UnsetRuntime
`func (o *BuildRequestWithContext) UnsetRuntime()`

UnsetRuntime ensures that no value is present for Runtime, not even an explicit nil
### GetTags

`func (o *BuildRequestWithContext) GetTags() []string`

GetTags returns the Tags field if non-nil, zero value otherwise.

### GetTagsOk

`func (o *BuildRequestWithContext) GetTagsOk() (*[]string, bool)`

GetTagsOk returns a tuple with the Tags field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTags

`func (o *BuildRequestWithContext) SetTags(v []string)`

SetTags sets Tags field to given value.

### HasTags

`func (o *BuildRequestWithContext) HasTags() bool`

HasTags returns a boolean if a field has been set.

### GetTarget

`func (o *BuildRequestWithContext) GetTarget() string`

GetTarget returns the Target field if non-nil, zero value otherwise.

### GetTargetOk

`func (o *BuildRequestWithContext) GetTargetOk() (*string, bool)`

GetTargetOk returns a tuple with the Target field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTarget

`func (o *BuildRequestWithContext) SetTarget(v string)`

SetTarget sets Target field to given value.

### HasTarget

`func (o *BuildRequestWithContext) HasTarget() bool`

HasTarget returns a boolean if a field has been set.

### SetTargetNil

`func (o *BuildRequestWithContext) SetTargetNil(b bool)`

 SetTargetNil sets the value for Target to be an explicit nil

### UnsetTarget
`func (o *BuildRequestWithContext) UnsetTarget()`

UnsetTarget ensures that no value is present for Target, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


