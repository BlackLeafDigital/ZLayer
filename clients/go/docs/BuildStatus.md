# BuildStatus

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CompletedAt** | Pointer to **NullableString** | When the build completed (ISO 8601) | [optional] 
**Error** | Pointer to **NullableString** | Error message (if failed) | [optional] 
**Id** | **string** | Unique build ID | 
**ImageId** | Pointer to **NullableString** | Image ID (if completed) | [optional] 
**StartedAt** | **string** | When the build started (ISO 8601) | 
**Status** | [**BuildStateEnum**](BuildStateEnum.md) | Current build status | 

## Methods

### NewBuildStatus

`func NewBuildStatus(id string, startedAt string, status BuildStateEnum, ) *BuildStatus`

NewBuildStatus instantiates a new BuildStatus object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewBuildStatusWithDefaults

`func NewBuildStatusWithDefaults() *BuildStatus`

NewBuildStatusWithDefaults instantiates a new BuildStatus object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCompletedAt

`func (o *BuildStatus) GetCompletedAt() string`

GetCompletedAt returns the CompletedAt field if non-nil, zero value otherwise.

### GetCompletedAtOk

`func (o *BuildStatus) GetCompletedAtOk() (*string, bool)`

GetCompletedAtOk returns a tuple with the CompletedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCompletedAt

`func (o *BuildStatus) SetCompletedAt(v string)`

SetCompletedAt sets CompletedAt field to given value.

### HasCompletedAt

`func (o *BuildStatus) HasCompletedAt() bool`

HasCompletedAt returns a boolean if a field has been set.

### SetCompletedAtNil

`func (o *BuildStatus) SetCompletedAtNil(b bool)`

 SetCompletedAtNil sets the value for CompletedAt to be an explicit nil

### UnsetCompletedAt
`func (o *BuildStatus) UnsetCompletedAt()`

UnsetCompletedAt ensures that no value is present for CompletedAt, not even an explicit nil
### GetError

`func (o *BuildStatus) GetError() string`

GetError returns the Error field if non-nil, zero value otherwise.

### GetErrorOk

`func (o *BuildStatus) GetErrorOk() (*string, bool)`

GetErrorOk returns a tuple with the Error field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetError

`func (o *BuildStatus) SetError(v string)`

SetError sets Error field to given value.

### HasError

`func (o *BuildStatus) HasError() bool`

HasError returns a boolean if a field has been set.

### SetErrorNil

`func (o *BuildStatus) SetErrorNil(b bool)`

 SetErrorNil sets the value for Error to be an explicit nil

### UnsetError
`func (o *BuildStatus) UnsetError()`

UnsetError ensures that no value is present for Error, not even an explicit nil
### GetId

`func (o *BuildStatus) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *BuildStatus) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *BuildStatus) SetId(v string)`

SetId sets Id field to given value.


### GetImageId

`func (o *BuildStatus) GetImageId() string`

GetImageId returns the ImageId field if non-nil, zero value otherwise.

### GetImageIdOk

`func (o *BuildStatus) GetImageIdOk() (*string, bool)`

GetImageIdOk returns a tuple with the ImageId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetImageId

`func (o *BuildStatus) SetImageId(v string)`

SetImageId sets ImageId field to given value.

### HasImageId

`func (o *BuildStatus) HasImageId() bool`

HasImageId returns a boolean if a field has been set.

### SetImageIdNil

`func (o *BuildStatus) SetImageIdNil(b bool)`

 SetImageIdNil sets the value for ImageId to be an explicit nil

### UnsetImageId
`func (o *BuildStatus) UnsetImageId()`

UnsetImageId ensures that no value is present for ImageId, not even an explicit nil
### GetStartedAt

`func (o *BuildStatus) GetStartedAt() string`

GetStartedAt returns the StartedAt field if non-nil, zero value otherwise.

### GetStartedAtOk

`func (o *BuildStatus) GetStartedAtOk() (*string, bool)`

GetStartedAtOk returns a tuple with the StartedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStartedAt

`func (o *BuildStatus) SetStartedAt(v string)`

SetStartedAt sets StartedAt field to given value.


### GetStatus

`func (o *BuildStatus) GetStatus() BuildStateEnum`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *BuildStatus) GetStatusOk() (*BuildStateEnum, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *BuildStatus) SetStatus(v BuildStateEnum)`

SetStatus sets Status field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


