# ReplicationInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Enabled** | **bool** | Whether replication is configured | 
**LastSync** | Pointer to **NullableString** | ISO-8601 timestamp of the last successful sync, or null | [optional] 
**PendingChanges** | **int32** | Number of WAL segments pending upload | 
**Status** | **string** | Current status: \&quot;active\&quot;, \&quot;disabled\&quot;, or \&quot;error\&quot; | 

## Methods

### NewReplicationInfo

`func NewReplicationInfo(enabled bool, pendingChanges int32, status string, ) *ReplicationInfo`

NewReplicationInfo instantiates a new ReplicationInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewReplicationInfoWithDefaults

`func NewReplicationInfoWithDefaults() *ReplicationInfo`

NewReplicationInfoWithDefaults instantiates a new ReplicationInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetEnabled

`func (o *ReplicationInfo) GetEnabled() bool`

GetEnabled returns the Enabled field if non-nil, zero value otherwise.

### GetEnabledOk

`func (o *ReplicationInfo) GetEnabledOk() (*bool, bool)`

GetEnabledOk returns a tuple with the Enabled field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEnabled

`func (o *ReplicationInfo) SetEnabled(v bool)`

SetEnabled sets Enabled field to given value.


### GetLastSync

`func (o *ReplicationInfo) GetLastSync() string`

GetLastSync returns the LastSync field if non-nil, zero value otherwise.

### GetLastSyncOk

`func (o *ReplicationInfo) GetLastSyncOk() (*string, bool)`

GetLastSyncOk returns a tuple with the LastSync field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastSync

`func (o *ReplicationInfo) SetLastSync(v string)`

SetLastSync sets LastSync field to given value.

### HasLastSync

`func (o *ReplicationInfo) HasLastSync() bool`

HasLastSync returns a boolean if a field has been set.

### SetLastSyncNil

`func (o *ReplicationInfo) SetLastSyncNil(b bool)`

 SetLastSyncNil sets the value for LastSync to be an explicit nil

### UnsetLastSync
`func (o *ReplicationInfo) UnsetLastSync()`

UnsetLastSync ensures that no value is present for LastSync, not even an explicit nil
### GetPendingChanges

`func (o *ReplicationInfo) GetPendingChanges() int32`

GetPendingChanges returns the PendingChanges field if non-nil, zero value otherwise.

### GetPendingChangesOk

`func (o *ReplicationInfo) GetPendingChangesOk() (*int32, bool)`

GetPendingChangesOk returns a tuple with the PendingChanges field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPendingChanges

`func (o *ReplicationInfo) SetPendingChanges(v int32)`

SetPendingChanges sets PendingChanges field to given value.


### GetStatus

`func (o *ReplicationInfo) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *ReplicationInfo) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *ReplicationInfo) SetStatus(v string)`

SetStatus sets Status field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


