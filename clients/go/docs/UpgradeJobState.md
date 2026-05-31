# UpgradeJobState

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Error** | Pointer to **NullableString** | Human-readable failure reason (set only when &#x60;status &#x3D;&#x3D; Failed&#x60;). | [optional] 
**FinishedAt** | Pointer to **NullableTime** | When the job entered its terminal state (&#x60;Restarting&#x60; or &#x60;Failed&#x60;). | [optional] 
**StartedAt** | **time.Time** | When the job was registered. | 
**Status** | [**UpgradeStatus**](UpgradeStatus.md) | Current lifecycle state. | 
**UpgradeId** | **string** | Server-generated upgrade id (UUID v4). | 
**Version** | Pointer to **NullableString** | Target version (e.g. &#x60;\&quot;v0.12.0\&quot;&#x60;); &#x60;None&#x60; means \&quot;latest release\&quot;. | [optional] 

## Methods

### NewUpgradeJobState

`func NewUpgradeJobState(startedAt time.Time, status UpgradeStatus, upgradeId string, ) *UpgradeJobState`

NewUpgradeJobState instantiates a new UpgradeJobState object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewUpgradeJobStateWithDefaults

`func NewUpgradeJobStateWithDefaults() *UpgradeJobState`

NewUpgradeJobStateWithDefaults instantiates a new UpgradeJobState object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetError

`func (o *UpgradeJobState) GetError() string`

GetError returns the Error field if non-nil, zero value otherwise.

### GetErrorOk

`func (o *UpgradeJobState) GetErrorOk() (*string, bool)`

GetErrorOk returns a tuple with the Error field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetError

`func (o *UpgradeJobState) SetError(v string)`

SetError sets Error field to given value.

### HasError

`func (o *UpgradeJobState) HasError() bool`

HasError returns a boolean if a field has been set.

### SetErrorNil

`func (o *UpgradeJobState) SetErrorNil(b bool)`

 SetErrorNil sets the value for Error to be an explicit nil

### UnsetError
`func (o *UpgradeJobState) UnsetError()`

UnsetError ensures that no value is present for Error, not even an explicit nil
### GetFinishedAt

`func (o *UpgradeJobState) GetFinishedAt() time.Time`

GetFinishedAt returns the FinishedAt field if non-nil, zero value otherwise.

### GetFinishedAtOk

`func (o *UpgradeJobState) GetFinishedAtOk() (*time.Time, bool)`

GetFinishedAtOk returns a tuple with the FinishedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetFinishedAt

`func (o *UpgradeJobState) SetFinishedAt(v time.Time)`

SetFinishedAt sets FinishedAt field to given value.

### HasFinishedAt

`func (o *UpgradeJobState) HasFinishedAt() bool`

HasFinishedAt returns a boolean if a field has been set.

### SetFinishedAtNil

`func (o *UpgradeJobState) SetFinishedAtNil(b bool)`

 SetFinishedAtNil sets the value for FinishedAt to be an explicit nil

### UnsetFinishedAt
`func (o *UpgradeJobState) UnsetFinishedAt()`

UnsetFinishedAt ensures that no value is present for FinishedAt, not even an explicit nil
### GetStartedAt

`func (o *UpgradeJobState) GetStartedAt() time.Time`

GetStartedAt returns the StartedAt field if non-nil, zero value otherwise.

### GetStartedAtOk

`func (o *UpgradeJobState) GetStartedAtOk() (*time.Time, bool)`

GetStartedAtOk returns a tuple with the StartedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStartedAt

`func (o *UpgradeJobState) SetStartedAt(v time.Time)`

SetStartedAt sets StartedAt field to given value.


### GetStatus

`func (o *UpgradeJobState) GetStatus() UpgradeStatus`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *UpgradeJobState) GetStatusOk() (*UpgradeStatus, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *UpgradeJobState) SetStatus(v UpgradeStatus)`

SetStatus sets Status field to given value.


### GetUpgradeId

`func (o *UpgradeJobState) GetUpgradeId() string`

GetUpgradeId returns the UpgradeId field if non-nil, zero value otherwise.

### GetUpgradeIdOk

`func (o *UpgradeJobState) GetUpgradeIdOk() (*string, bool)`

GetUpgradeIdOk returns a tuple with the UpgradeId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUpgradeId

`func (o *UpgradeJobState) SetUpgradeId(v string)`

SetUpgradeId sets UpgradeId field to given value.


### GetVersion

`func (o *UpgradeJobState) GetVersion() string`

GetVersion returns the Version field if non-nil, zero value otherwise.

### GetVersionOk

`func (o *UpgradeJobState) GetVersionOk() (*string, bool)`

GetVersionOk returns a tuple with the Version field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetVersion

`func (o *UpgradeJobState) SetVersion(v string)`

SetVersion sets Version field to given value.

### HasVersion

`func (o *UpgradeJobState) HasVersion() bool`

HasVersion returns a boolean if a field has been set.

### SetVersionNil

`func (o *UpgradeJobState) SetVersionNil(b bool)`

 SetVersionNil sets the value for Version to be an explicit nil

### UnsetVersion
`func (o *UpgradeJobState) UnsetVersion()`

UnsetVersion ensures that no value is present for Version, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


