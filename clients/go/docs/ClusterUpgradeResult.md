# ClusterUpgradeResult

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Errors** | [**[]UpgradeError**](UpgradeError.md) | Nodes that failed mid-upgrade (one entry per failure). | 
**Skipped** | **[]int64** | Nodes intentionally skipped (e.g. unreachable, missing api endpoint). | 
**Upgraded** | **[]int64** | Nodes that successfully restarted on the new binary. | 

## Methods

### NewClusterUpgradeResult

`func NewClusterUpgradeResult(errors []UpgradeError, skipped []int64, upgraded []int64, ) *ClusterUpgradeResult`

NewClusterUpgradeResult instantiates a new ClusterUpgradeResult object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewClusterUpgradeResultWithDefaults

`func NewClusterUpgradeResultWithDefaults() *ClusterUpgradeResult`

NewClusterUpgradeResultWithDefaults instantiates a new ClusterUpgradeResult object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetErrors

`func (o *ClusterUpgradeResult) GetErrors() []UpgradeError`

GetErrors returns the Errors field if non-nil, zero value otherwise.

### GetErrorsOk

`func (o *ClusterUpgradeResult) GetErrorsOk() (*[]UpgradeError, bool)`

GetErrorsOk returns a tuple with the Errors field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetErrors

`func (o *ClusterUpgradeResult) SetErrors(v []UpgradeError)`

SetErrors sets Errors field to given value.


### GetSkipped

`func (o *ClusterUpgradeResult) GetSkipped() []int64`

GetSkipped returns the Skipped field if non-nil, zero value otherwise.

### GetSkippedOk

`func (o *ClusterUpgradeResult) GetSkippedOk() (*[]int64, bool)`

GetSkippedOk returns a tuple with the Skipped field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSkipped

`func (o *ClusterUpgradeResult) SetSkipped(v []int64)`

SetSkipped sets Skipped field to given value.


### GetUpgraded

`func (o *ClusterUpgradeResult) GetUpgraded() []int64`

GetUpgraded returns the Upgraded field if non-nil, zero value otherwise.

### GetUpgradedOk

`func (o *ClusterUpgradeResult) GetUpgradedOk() (*[]int64, bool)`

GetUpgradedOk returns a tuple with the Upgraded field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUpgraded

`func (o *ClusterUpgradeResult) SetUpgraded(v []int64)`

SetUpgraded sets Upgraded field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


