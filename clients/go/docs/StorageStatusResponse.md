# StorageStatusResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Replication** | [**ReplicationInfo**](ReplicationInfo.md) | Replication status details | 

## Methods

### NewStorageStatusResponse

`func NewStorageStatusResponse(replication ReplicationInfo, ) *StorageStatusResponse`

NewStorageStatusResponse instantiates a new StorageStatusResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewStorageStatusResponseWithDefaults

`func NewStorageStatusResponseWithDefaults() *StorageStatusResponse`

NewStorageStatusResponseWithDefaults instantiates a new StorageStatusResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetReplication

`func (o *StorageStatusResponse) GetReplication() ReplicationInfo`

GetReplication returns the Replication field if non-nil, zero value otherwise.

### GetReplicationOk

`func (o *StorageStatusResponse) GetReplicationOk() (*ReplicationInfo, bool)`

GetReplicationOk returns a tuple with the Replication field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReplication

`func (o *StorageStatusResponse) SetReplication(v ReplicationInfo)`

SetReplication sets Replication field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


