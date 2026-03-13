# BuildRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**BuildArgs** | Pointer to **map[string]string** | Build arguments (ARG values) | [optional] 
**NoCache** | Pointer to **bool** | Disable cache | [optional] 
**Push** | Pointer to **bool** | Push to registry after build | [optional] 
**Runtime** | Pointer to **NullableString** | Use runtime template instead of Dockerfile | [optional] 
**Tags** | Pointer to **[]string** | Tags to apply to the image | [optional] 
**Target** | Pointer to **NullableString** | Target stage for multi-stage builds | [optional] 

## Methods

### NewBuildRequest

`func NewBuildRequest() *BuildRequest`

NewBuildRequest instantiates a new BuildRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewBuildRequestWithDefaults

`func NewBuildRequestWithDefaults() *BuildRequest`

NewBuildRequestWithDefaults instantiates a new BuildRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetBuildArgs

`func (o *BuildRequest) GetBuildArgs() map[string]string`

GetBuildArgs returns the BuildArgs field if non-nil, zero value otherwise.

### GetBuildArgsOk

`func (o *BuildRequest) GetBuildArgsOk() (*map[string]string, bool)`

GetBuildArgsOk returns a tuple with the BuildArgs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBuildArgs

`func (o *BuildRequest) SetBuildArgs(v map[string]string)`

SetBuildArgs sets BuildArgs field to given value.

### HasBuildArgs

`func (o *BuildRequest) HasBuildArgs() bool`

HasBuildArgs returns a boolean if a field has been set.

### GetNoCache

`func (o *BuildRequest) GetNoCache() bool`

GetNoCache returns the NoCache field if non-nil, zero value otherwise.

### GetNoCacheOk

`func (o *BuildRequest) GetNoCacheOk() (*bool, bool)`

GetNoCacheOk returns a tuple with the NoCache field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNoCache

`func (o *BuildRequest) SetNoCache(v bool)`

SetNoCache sets NoCache field to given value.

### HasNoCache

`func (o *BuildRequest) HasNoCache() bool`

HasNoCache returns a boolean if a field has been set.

### GetPush

`func (o *BuildRequest) GetPush() bool`

GetPush returns the Push field if non-nil, zero value otherwise.

### GetPushOk

`func (o *BuildRequest) GetPushOk() (*bool, bool)`

GetPushOk returns a tuple with the Push field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPush

`func (o *BuildRequest) SetPush(v bool)`

SetPush sets Push field to given value.

### HasPush

`func (o *BuildRequest) HasPush() bool`

HasPush returns a boolean if a field has been set.

### GetRuntime

`func (o *BuildRequest) GetRuntime() string`

GetRuntime returns the Runtime field if non-nil, zero value otherwise.

### GetRuntimeOk

`func (o *BuildRequest) GetRuntimeOk() (*string, bool)`

GetRuntimeOk returns a tuple with the Runtime field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRuntime

`func (o *BuildRequest) SetRuntime(v string)`

SetRuntime sets Runtime field to given value.

### HasRuntime

`func (o *BuildRequest) HasRuntime() bool`

HasRuntime returns a boolean if a field has been set.

### SetRuntimeNil

`func (o *BuildRequest) SetRuntimeNil(b bool)`

 SetRuntimeNil sets the value for Runtime to be an explicit nil

### UnsetRuntime
`func (o *BuildRequest) UnsetRuntime()`

UnsetRuntime ensures that no value is present for Runtime, not even an explicit nil
### GetTags

`func (o *BuildRequest) GetTags() []string`

GetTags returns the Tags field if non-nil, zero value otherwise.

### GetTagsOk

`func (o *BuildRequest) GetTagsOk() (*[]string, bool)`

GetTagsOk returns a tuple with the Tags field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTags

`func (o *BuildRequest) SetTags(v []string)`

SetTags sets Tags field to given value.

### HasTags

`func (o *BuildRequest) HasTags() bool`

HasTags returns a boolean if a field has been set.

### GetTarget

`func (o *BuildRequest) GetTarget() string`

GetTarget returns the Target field if non-nil, zero value otherwise.

### GetTargetOk

`func (o *BuildRequest) GetTargetOk() (*string, bool)`

GetTargetOk returns a tuple with the Target field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTarget

`func (o *BuildRequest) SetTarget(v string)`

SetTarget sets Target field to given value.

### HasTarget

`func (o *BuildRequest) HasTarget() bool`

HasTarget returns a boolean if a field has been set.

### SetTargetNil

`func (o *BuildRequest) SetTargetNil(b bool)`

 SetTargetNil sets the value for Target to be an explicit nil

### UnsetTarget
`func (o *BuildRequest) UnsetTarget()`

UnsetTarget ensures that no value is present for Target, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


