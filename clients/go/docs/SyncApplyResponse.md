# SyncApplyResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AppliedSha** | Pointer to **NullableString** | Commit SHA the sync was applied against (when resolvable). | [optional] 
**Results** | [**[]SyncResourceResult**](SyncResourceResult.md) | Per-resource reconcile results. | 
**Summary** | **string** | Aggregate summary suitable for CLI output, e.g. &#x60;\&quot;3 created, 2 updated, 1 deleted, 0 skipped\&quot;&#x60;. | 

## Methods

### NewSyncApplyResponse

`func NewSyncApplyResponse(results []SyncResourceResult, summary string, ) *SyncApplyResponse`

NewSyncApplyResponse instantiates a new SyncApplyResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewSyncApplyResponseWithDefaults

`func NewSyncApplyResponseWithDefaults() *SyncApplyResponse`

NewSyncApplyResponseWithDefaults instantiates a new SyncApplyResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAppliedSha

`func (o *SyncApplyResponse) GetAppliedSha() string`

GetAppliedSha returns the AppliedSha field if non-nil, zero value otherwise.

### GetAppliedShaOk

`func (o *SyncApplyResponse) GetAppliedShaOk() (*string, bool)`

GetAppliedShaOk returns a tuple with the AppliedSha field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAppliedSha

`func (o *SyncApplyResponse) SetAppliedSha(v string)`

SetAppliedSha sets AppliedSha field to given value.

### HasAppliedSha

`func (o *SyncApplyResponse) HasAppliedSha() bool`

HasAppliedSha returns a boolean if a field has been set.

### SetAppliedShaNil

`func (o *SyncApplyResponse) SetAppliedShaNil(b bool)`

 SetAppliedShaNil sets the value for AppliedSha to be an explicit nil

### UnsetAppliedSha
`func (o *SyncApplyResponse) UnsetAppliedSha()`

UnsetAppliedSha ensures that no value is present for AppliedSha, not even an explicit nil
### GetResults

`func (o *SyncApplyResponse) GetResults() []SyncResourceResult`

GetResults returns the Results field if non-nil, zero value otherwise.

### GetResultsOk

`func (o *SyncApplyResponse) GetResultsOk() (*[]SyncResourceResult, bool)`

GetResultsOk returns a tuple with the Results field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResults

`func (o *SyncApplyResponse) SetResults(v []SyncResourceResult)`

SetResults sets Results field to given value.


### GetSummary

`func (o *SyncApplyResponse) GetSummary() string`

GetSummary returns the Summary field if non-nil, zero value otherwise.

### GetSummaryOk

`func (o *SyncApplyResponse) GetSummaryOk() (*string, bool)`

GetSummaryOk returns a tuple with the Summary field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSummary

`func (o *SyncApplyResponse) SetSummary(v string)`

SetSummary sets Summary field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


